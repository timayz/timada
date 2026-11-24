use std::sync::Mutex;

use evento::Evento;
use timada_google::{
    CATCH_UP_SECS, CHECK_EVERY_SECS, last_search_day, search_totals, top_pages, top_queries,
};
use timada_tenancy::CreateTenant;

use super::*;
use crate::google::tests::{Request, StandIn, connected, token_answer};

const SITE: &str = "sc-domain:shop.example";

/// What the stand-in's Search Console holds and how it answers.
#[derive(Default)]
struct Held {
    /// The days Google has finished counting.
    days: Vec<String>,
    /// The status every question about the numbers gets, when not 200.
    failing: Option<u16>,
    /// The questions about the rows of a day fail from this many on.
    rows_fail_after: Option<usize>,
    rows_asked: usize,
    /// The grant is withdrawn: no access is given any more.
    withdrawn: bool,
}

/// The questions about the site's numbers, as `(first, last, grouping)`.
fn questions(stand_in: &StandIn) -> Vec<(String, String, String)> {
    stand_in
        .requests
        .lock()
        .unwrap()
        .iter()
        .filter(|request| request.line.contains("/searchAnalytics/query"))
        .map(|request| {
            let body: serde_json::Value = serde_json::from_str(&request.body).unwrap();
            assert_eq!(body["type"], "web");
            assert_eq!(body["dataState"], "final");
            // Never by query and by page at once.
            assert_eq!(body["dimensions"].as_array().unwrap().len(), 1);
            (
                body["startDate"].as_str().unwrap().to_owned(),
                body["endDate"].as_str().unwrap().to_owned(),
                body["dimensions"][0].as_str().unwrap().to_owned(),
            )
        })
        .collect()
}

fn answer(held: &Mutex<Held>, request: &Request) -> (u16, String) {
    let mut held = held.lock().unwrap();
    if request.line.starts_with("POST /token") {
        return if held.withdrawn {
            (400, r#"{"error":"invalid_grant"}"#.into())
        } else {
            token_answer("access-1")
        };
    }
    assert_eq!(
        request.line,
        "POST /webmasters/v3/sites/sc-domain:shop.example/searchAnalytics/query HTTP/1.1"
    );
    assert_eq!(request.authorization, "Bearer access-1");
    if let Some(status) = held.failing {
        return (status, r#"{"error":{"message":"Backend Error"}}"#.into());
    }
    let body: serde_json::Value = serde_json::from_str(&request.body).unwrap();
    let (first, last) = (
        body["startDate"].as_str().unwrap(),
        body["endDate"].as_str().unwrap(),
    );
    let rows: Vec<serde_json::Value> = match body["dimensions"][0].as_str().unwrap() {
        "date" => held
            .days
            .iter()
            .filter(|day| day.as_str() >= first && day.as_str() <= last)
            .map(|day| {
                serde_json::json!({
                    "keys": [day], "clicks": 10.0, "impressions": 200.0,
                    "ctr": 0.05, "position": 8.25,
                })
            })
            .collect(),
        grouping => {
            held.rows_asked += 1;
            if held
                .rows_fail_after
                .is_some_and(|after| held.rows_asked > after)
            {
                return (429, r#"{"error":{"message":"Quota exceeded"}}"#.into());
            }
            assert_eq!(first, last);
            assert_eq!(body["rowLimit"], MAX_SEARCH_ROWS);
            if grouping == "query" {
                vec![
                    serde_json::json!({
                        "keys": ["arbre à chat"], "clicks": 6.0, "impressions": 100.0,
                        "ctr": 0.06, "position": 4.0,
                    }),
                    serde_json::json!({
                        "keys": ["griffoir"], "clicks": 2.0, "impressions": 50.0,
                        "ctr": 0.04, "position": 12.5,
                    }),
                ]
            } else {
                vec![serde_json::json!({
                    "keys": ["https://shop.example/arbres-a-chat"], "clicks": 9.0,
                    "impressions": 180.0, "ctr": 0.05, "position": 7.5,
                })]
            }
        }
    };
    // Google leaves `rows` out when there is none.
    if rows.is_empty() {
        (200, r#"{"responseAggregationType":"byProperty"}"#.into())
    } else {
        (200, serde_json::json!({ "rows": rows }).to_string())
    }
}

struct Rig {
    turso: evento::Turso,
    store: Evento,
    db: Db,
    shop: String,
    held: Arc<Mutex<Held>>,
    stand_in: StandIn,
    google: Google,
    now: u64,
    today: String,
}

impl Rig {
    /// A shop connected to Google, its site chosen, and a stand-in where
    /// Google has finished the `finished` days before the last two.
    async fn new(finished: u32) -> Self {
        let turso = evento::Turso::in_memory_concurrent().await.unwrap();
        crate::db::migrate_store_read_models(&turso).await.unwrap();
        let db = Db::from(turso.clone());
        let store = Evento::new(turso.clone());
        let shop = timada_tenancy::Command(store.clone())
            .create_tenant(CreateTenant {
                name: "acme".to_owned(),
                owner_user_id: "alice".to_owned(),
            })
            .await
            .unwrap()
            .id;
        let now = crate::auth::now();
        let today = format_date(now);
        let held = Arc::new(Mutex::new(Held {
            days: (0..finished)
                .map(|back| day_before(&today, back + 2))
                .collect(),
            ..Held::default()
        }));
        let stand_in = StandIn::start({
            let held = held.clone();
            move |request| answer(&held, request)
        })
        .await;
        let google = stand_in.google();
        let rig = Self {
            turso,
            store,
            db,
            shop,
            held,
            stand_in,
            google,
            now,
            today,
        };
        rig.connect("refresh-1").await;
        rig.command()
            .choose_site("alice", &rig.shop, SITE)
            .await
            .unwrap();
        rig.catch_up().await;
        rig
    }

    async fn connect(&self, refresh_token: &str) {
        connected(&self.turso, &self.google, &self.shop, refresh_token).await;
    }

    fn command(&self) -> Command<Evento> {
        Command(self.store.clone())
    }

    fn node(&self, share: Share) -> Arc<SearchPasses> {
        SearchPasses::new(
            self.db.clone(),
            self.store.clone(),
            Arc::new(move || share),
            self.google.clone(),
        )
    }

    fn only(&self) -> Arc<SearchPasses> {
        self.node(Share { index: 0, of: 1 })
    }

    /// The read model the sweep reads, brought up to date.
    async fn catch_up(&self) {
        timada_google::read_model_subscription::<Evento>(self.db.clone())
            .run_once(&self.store)
            .await
            .unwrap();
    }

    async fn watch(&self) -> SearchWatchState {
        self.command()
            .load_search_watch(&self.shop)
            .await
            .unwrap()
            .unwrap()
    }

    async fn held_days(&self) -> Vec<String> {
        search_days_held(&self.db, &self.shop, SITE, "2000-01-01")
            .await
            .unwrap()
    }
}

/// A shop just connected is read for the days Google has finished, each
/// with its queries and its pages, and not again until the next day.
#[tokio::test]
async fn a_connected_shop_is_read_once_a_day() {
    let rig = Rig::new(3).await;
    assert_eq!(rig.only().sweep(rig.now).await.unwrap(), 3);
    rig.catch_up().await;

    // One question for the days, over the ninety a new shop is owed, then
    // two a day, the newest day first.
    let asked = questions(&rig.stand_in);
    assert_eq!(
        asked[0],
        (
            day_before(&rig.today, BACKFILL_DAYS),
            rig.today.clone(),
            "date".to_owned()
        )
    );
    let newest = day_before(&rig.today, 2);
    assert_eq!(
        asked[1],
        (newest.clone(), newest.clone(), "query".to_owned())
    );
    assert_eq!(
        asked[2],
        (newest.clone(), newest.clone(), "page".to_owned())
    );
    assert_eq!(asked.len(), 7);
    assert_eq!(rig.stand_in.renewals(), 1);

    assert_eq!(
        rig.held_days().await,
        vec![
            day_before(&rig.today, 4),
            day_before(&rig.today, 3),
            newest.clone()
        ]
    );
    assert_eq!(
        last_search_day(&rig.db, &rig.shop, SITE).await.unwrap(),
        Some(newest.clone())
    );
    let totals = search_totals(&rig.db, &rig.shop, SITE, "2000-01-01", &rig.today)
        .await
        .unwrap();
    assert_eq!(
        (totals.days, totals.clicks, totals.impressions),
        (3, 30, 600)
    );
    // Position 8.25 every day.
    assert_eq!(totals.figures().position_tenths(), Some(83));
    let queries = top_queries(&rig.db, &rig.shop, SITE, &newest, &newest, 10)
        .await
        .unwrap();
    assert_eq!(queries.len(), 2);
    assert_eq!(
        (queries[0].key.as_str(), queries[0].clicks),
        ("arbre à chat", 6)
    );
    assert_eq!(queries[1].figures().position_tenths(), Some(125));
    let pages = top_pages(&rig.db, &rig.shop, SITE, &newest, &newest, 10)
        .await
        .unwrap();
    assert_eq!(pages[0].key, "https://shop.example/arbres-a-chat");

    // The pass ended: the next is due a day later, and none before.
    let watch = rig.watch().await;
    assert!(!watch.behind);
    assert_eq!(watch.last_error, None);
    assert_eq!(watch.next_at, watch.checked_at + CHECK_EVERY_SECS);
    assert_eq!(rig.only().sweep(watch.next_at - 1).await.unwrap(), 0);
    assert_eq!(questions(&rig.stand_in).len(), 7);

    // A day later Google has finished one more: only the last days are
    // asked about, and only the new one fetched.
    let next = watch.next_at;
    let tomorrow = format_date(next);
    rig.held
        .lock()
        .unwrap()
        .days
        .push(day_before(&rig.today, 1));
    assert_eq!(rig.only().sweep(next).await.unwrap(), 1);
    let asked = questions(&rig.stand_in);
    assert_eq!(
        asked[7],
        (
            day_before(&tomorrow, RECENT_DAYS),
            tomorrow.clone(),
            "date".to_owned()
        )
    );
    assert_eq!(asked.len(), 10);
    rig.catch_up().await;
    assert_eq!(rig.held_days().await.len(), 4);
    // The access held served the second pass too.
    assert_eq!(rig.stand_in.renewals(), 1);
}

/// The days owed to a new shop come a few a pass, the newest first, with
/// the next pass soon while older ones are left.
#[tokio::test]
async fn the_days_owed_are_fetched_a_few_a_pass() {
    let rig = Rig::new(25).await;
    assert_eq!(rig.only().sweep(rig.now).await.unwrap(), DAYS_A_PASS);
    rig.catch_up().await;
    let watch = rig.watch().await;
    assert!(watch.behind);
    assert_eq!(watch.next_at, watch.checked_at + CATCH_UP_SECS);
    // The newest ten.
    assert_eq!(
        rig.held_days().await.first(),
        Some(&day_before(&rig.today, 11))
    );

    assert_eq!(rig.only().sweep(watch.next_at).await.unwrap(), DAYS_A_PASS);
    rig.catch_up().await;
    let watch = rig.watch().await;
    assert!(watch.behind);
    assert_eq!(rig.only().sweep(watch.next_at).await.unwrap(), 5);
    rig.catch_up().await;
    let watch = rig.watch().await;
    assert!(!watch.behind);
    assert_eq!(rig.held_days().await.len(), 25);
    assert_eq!(watch.next_at, watch.checked_at + CHECK_EVERY_SECS);
}

/// Two nodes that both think a shop is theirs, their read models a moment
/// behind: the days are recorded once.
#[tokio::test]
async fn two_nodes_reading_the_same_shop_record_its_days_once() {
    let rig = Rig::new(2).await;
    let (a, b) = (rig.only(), rig.only());
    let (first, second) = tokio::join!(a.sweep(rig.now), b.sweep(rig.now));
    assert_eq!(first.unwrap() + second.unwrap(), 2);
    rig.catch_up().await;
    assert_eq!(rig.held_days().await.len(), 2);
    let totals = search_totals(&rig.db, &rig.shop, SITE, "2000-01-01", &rig.today)
        .await
        .unwrap();
    assert_eq!(totals.clicks, 20);
}

/// Each node reads for its own part of the shops.
#[tokio::test]
async fn a_node_leaves_the_other_nodes_shops_alone() {
    let rig = Rig::new(2).await;
    let parts = [Share { index: 0, of: 2 }, Share { index: 1, of: 2 }];
    let theirs = parts
        .into_iter()
        .find(|share| !share.owns(&rig.shop))
        .unwrap();
    assert_eq!(rig.node(theirs).sweep(rig.now).await.unwrap(), 0);
    assert!(questions(&rig.stand_in).is_empty());
    let mine = parts
        .into_iter()
        .find(|share| share.owns(&rig.shop))
        .unwrap();
    assert_eq!(rig.node(mine).sweep(rig.now).await.unwrap(), 2);
}

/// A pass Google does not answer is recorded with its reason and tried
/// again later, further apart each time; one that works clears it.
#[tokio::test]
async fn a_pass_that_fails_is_recorded_and_tried_again() {
    let rig = Rig::new(3).await;
    rig.held.lock().unwrap().failing = Some(500);
    assert_eq!(rig.only().sweep(rig.now).await.unwrap(), 0);
    rig.catch_up().await;
    let watch = rig.watch().await;
    assert_eq!(watch.attempts, 1);
    assert_eq!(
        watch.last_error.as_deref(),
        Some("Google is unavailable: Backend Error")
    );
    assert_eq!(watch.checked_at, 0);
    // Not before an hour.
    assert_eq!(rig.only().sweep(watch.next_at - 1).await.unwrap(), 0);
    assert_eq!(questions(&rig.stand_in).len(), 1);
    assert!(watch.next_at >= rig.now + 3_600);

    let first_retry = watch.next_at;
    assert_eq!(rig.only().sweep(first_retry).await.unwrap(), 0);
    rig.catch_up().await;
    let watch = rig.watch().await;
    assert_eq!(watch.attempts, 2);
    // Two hours after the second failure, which the event's own time
    // counts from.
    assert!(watch.next_at >= rig.now + 2 * 3_600);

    rig.held.lock().unwrap().failing = None;
    assert_eq!(rig.only().sweep(watch.next_at).await.unwrap(), 3);
    let watch = rig.watch().await;
    assert_eq!(watch.attempts, 0);
    assert_eq!(watch.last_error, None);
    assert!(watch.checked_at > 0);
}

/// A pass that fails half-way keeps the days it recorded: the next one
/// fetches the rest.
#[tokio::test]
async fn the_days_read_before_a_failure_stand() {
    let rig = Rig::new(3).await;
    // The first day's two questions are answered, the next is not.
    rig.held.lock().unwrap().rows_fail_after = Some(2);
    assert_eq!(rig.only().sweep(rig.now).await.unwrap(), 1);
    rig.catch_up().await;
    assert_eq!(rig.held_days().await, vec![day_before(&rig.today, 2)]);
    let watch = rig.watch().await;
    assert_eq!(
        watch.last_error.as_deref(),
        Some("Google is unavailable: Quota exceeded")
    );

    rig.held.lock().unwrap().rows_fail_after = None;
    assert_eq!(rig.only().sweep(watch.next_at).await.unwrap(), 2);
    rig.catch_up().await;
    assert_eq!(rig.held_days().await.len(), 3);
    // The day held was not fetched again: one question for the days, then
    // two for each of the two missing.
    let asked = questions(&rig.stand_in);
    let again = &asked[4..];
    assert_eq!(again.len(), 5);
    assert!(
        again.iter().all(|(first, _, grouping)| {
            grouping == "date" || *first != day_before(&rig.today, 2)
        })
    );
}

/// A grant Google no longer honours is recorded on the connection, not as a
/// failed pass, and the shop is left alone until its member connects again.
#[tokio::test]
async fn a_withdrawn_grant_shows_as_connect_again_and_stops_the_passes() {
    let rig = Rig::new(3).await;
    rig.held.lock().unwrap().withdrawn = true;
    assert_eq!(rig.only().sweep(rig.now).await.unwrap(), 0);
    let connection = rig
        .command()
        .load_connection(&rig.shop)
        .await
        .unwrap()
        .unwrap();
    assert!(connection.lapsed);
    assert!(
        rig.command()
            .load_search_watch(&rig.shop)
            .await
            .unwrap()
            .is_none()
    );
    rig.catch_up().await;
    assert!(
        searches_due(&rig.db, i64::MAX, "", 10)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        rig.only()
            .sweep(rig.now + 10 * CHECK_EVERY_SECS)
            .await
            .unwrap(),
        0
    );
    assert_eq!(rig.stand_in.renewals(), 1);

    // Connected again: read at the next sweep.
    rig.held.lock().unwrap().withdrawn = false;
    rig.connect("refresh-2").await;
    rig.catch_up().await;
    assert_eq!(rig.only().sweep(rig.now).await.unwrap(), 3);
}

/// A shop that took its connection off is left alone, and keeps what was
/// read.
#[tokio::test]
async fn a_disconnected_shop_is_left_alone_and_keeps_what_was_read() {
    let rig = Rig::new(2).await;
    assert_eq!(rig.only().sweep(rig.now).await.unwrap(), 2);
    rig.catch_up().await;
    rig.command().disconnect("alice", &rig.shop).await.unwrap();
    rig.catch_up().await;
    assert_eq!(
        rig.only()
            .sweep(rig.now + 2 * CHECK_EVERY_SECS)
            .await
            .unwrap(),
        0
    );
    assert_eq!(questions(&rig.stand_in).len(), 5);
    assert_eq!(rig.held_days().await.len(), 2);
}

/// A suspended shop is not read for.
#[tokio::test]
async fn a_suspended_shop_is_left_alone() {
    let rig = Rig::new(2).await;
    timada_tenancy::Command(rig.store.clone())
        .suspend_tenant(&rig.shop, "root")
        .await
        .unwrap();
    assert_eq!(rig.only().sweep(rig.now).await.unwrap(), 0);
    assert!(questions(&rig.stand_in).is_empty());
}
