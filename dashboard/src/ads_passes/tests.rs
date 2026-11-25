use std::sync::Mutex;

use evento::Evento;
use timada_google::{
    CATCH_UP_SECS, CHECK_EVERY_SECS, ads_by_product, ads_by_term, ads_totals, last_ads_day,
};
use timada_tenancy::CreateTenant;

use super::*;
use crate::google::tests::{Request, StandIn, connected, token_answer};

const ACCOUNT: &str = "1234567890";
const ZONE: &str = "Europe/Paris";

/// What the stand-in's Google Ads holds and how it answers.
#[derive(Default)]
struct Held {
    /// The days the account's ads were shown on.
    days: Vec<String>,
    /// What a day's conversions were worth, in the account's currency.
    value: f64,
    /// The status every question gets, when not 200.
    failing: Option<u16>,
    /// The questions fail from this many on.
    fail_after: Option<usize>,
    asked: usize,
    /// The grant is withdrawn: no access is given any more.
    withdrawn: bool,
}

/// The questions asked of Google Ads, as `(report, first, last)`.
fn questions(stand_in: &StandIn) -> Vec<(String, String, String)> {
    stand_in
        .requests
        .lock()
        .unwrap()
        .iter()
        .filter(|request| request.line.contains("googleAds:searchStream"))
        .map(|request| {
            let body: serde_json::Value = serde_json::from_str(&request.body).unwrap();
            let query = body["query"].as_str().unwrap();
            let report = query
                .split(" FROM ")
                .nth(1)
                .and_then(|rest| rest.split(' ').next())
                .unwrap();
            let days: Vec<&str> = query.split('\'').collect();
            (report.to_owned(), days[1].to_owned(), days[3].to_owned())
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
        "POST /ads/v26/customers/1234567890/googleAds:searchStream HTTP/1.1"
    );
    assert_eq!(request.authorization, "Bearer access-1");
    held.asked += 1;
    if let Some(status) = held.failing {
        return (status, r#"{"error":{"message":"Backend Error"}}"#.into());
    }
    if held.fail_after.is_some_and(|after| held.asked > after) {
        return (
            429,
            r#"[{"error":{"message":"Quota","details":[{"errors":[{"errorCode":{"quotaError":"RESOURCE_EXHAUSTED"},"message":"Too many requests"}]}]}}]"#.into(),
        );
    }
    let body: serde_json::Value = serde_json::from_str(&request.body).unwrap();
    let query = body["query"].as_str().unwrap();
    let days: Vec<&str> = query.split('\'').collect();
    let (first, last) = (days[1], days[3]);
    let metrics = serde_json::json!({
        "costMicros": "12000000", "impressions": "2000", "clicks": "40",
        "conversions": 2.0, "conversionsValue": held.value,
    });
    let results: Vec<serde_json::Value> = held
        .days
        .iter()
        .filter(|day| day.as_str() >= first && day.as_str() <= last)
        .map(|day| {
            if query.contains("FROM campaign ") {
                serde_json::json!({
                    "campaign": {"id": "11", "name": "Shopping"},
                    "customer": {"currencyCode": "EUR"},
                    "segments": {"date": day, "device": "MOBILE"},
                    "metrics": metrics,
                })
            } else if query.contains("FROM ad_group ") {
                serde_json::json!({
                    "campaign": {"id": "11"},
                    "adGroup": {"id": "21", "name": "Arbres à chat"},
                    "segments": {"date": day},
                    "metrics": metrics,
                })
            } else if query.contains("FROM shopping_performance_view ") {
                serde_json::json!({
                    "campaign": {"id": "11"},
                    "adGroup": {"id": "21"},
                    "segments": {"date": day, "productItemId": "01arbre"},
                    "metrics": metrics,
                })
            } else {
                serde_json::json!({
                    "searchTermView": {"searchTerm": "arbre à chat"},
                    "campaign": {"id": "11"},
                    "adGroup": {"id": "21"},
                    "segments": {"date": day},
                    "metrics": metrics,
                })
            }
        })
        .collect();
    (200, serde_json::json!([{ "results": results }]).to_string())
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
    /// The account's yesterday.
    last: String,
}

impl Rig {
    /// A shop connected to Google, its Google Ads account chosen, and a
    /// stand-in where its ads were shown on each of the last `shown` days.
    async fn new(shown: u32) -> Self {
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
        let last = last_finished_day(now, ZONE);
        let held = Arc::new(Mutex::new(Held {
            days: (0..shown).map(|back| day_before(&last, back)).collect(),
            value: 40.0,
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
            last,
        };
        rig.connect("refresh-1").await;
        rig.command()
            .choose_ads_account(
                "alice",
                &rig.shop,
                AdsAccount {
                    customer_id: ACCOUNT.to_owned(),
                    login_customer_id: None,
                    name: "Acme".to_owned(),
                    currency: "EUR".to_owned(),
                    time_zone: ZONE.to_owned(),
                },
            )
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

    fn node(&self, share: Share, calls_a_day: u32) -> Arc<AdsPasses> {
        AdsPasses::new(
            self.db.clone(),
            self.store.clone(),
            Arc::new(move || share),
            self.google.clone(),
            calls_a_day,
        )
    }

    fn only(&self) -> Arc<AdsPasses> {
        self.node(Share { index: 0, of: 1 }, 2_880)
    }

    /// The read model the sweep reads, brought up to date.
    async fn catch_up(&self) {
        timada_google::read_model_subscription::<Evento>(self.db.clone())
            .run_once(&self.store)
            .await
            .unwrap();
    }

    async fn watch(&self) -> AdsWatchState {
        self.command()
            .load_ads_watch(&self.shop)
            .await
            .unwrap()
            .unwrap()
    }

    async fn days_held(&self) -> i64 {
        ads_totals(&self.db, &self.shop, ACCOUNT, "2000-01-01", "2999-01-01")
            .await
            .unwrap()
            .days
    }
}

#[test]
fn a_day_is_over_on_the_accounts_own_clock() {
    // 2026-11-20 22:30 UTC: the 21st has not begun in Paris yet.
    let now = 1_795_213_800;
    assert_eq!(format_date(now), "2026-11-20");
    assert_eq!(last_finished_day(now, "Europe/Paris"), "2026-11-19");
    // An hour later it has, and not in New York.
    assert_eq!(last_finished_day(now + 3_600, "Europe/Paris"), "2026-11-20");
    assert_eq!(
        last_finished_day(now + 3_600, "America/New_York"),
        "2026-11-19"
    );
    assert_eq!(last_finished_day(now, ""), "2026-11-19");
    assert_eq!(last_finished_day(now, "Mars/Olympus"), "2026-11-19");
}

#[test]
fn the_days_a_pass_reads_follow_from_where_the_read_stands() {
    let last = "2026-11-20";
    // Never read: the last month, the rest owed.
    assert_eq!(
        days_to_read(last, None),
        ("2026-10-21".to_owned(), last.to_owned(), true)
    );
    let watch = |read_from: &str, read_to: &str, behind: bool| AdsWatchState {
        checked_at: 1,
        read_from: read_from.to_owned(),
        read_to: read_to.to_owned(),
        behind,
        ..AdsWatchState::default()
    };
    // Owed: the month before, then what is left of the ninety days.
    assert_eq!(
        days_to_read(last, Some(&watch("2026-10-21", last, true))),
        ("2026-09-20".to_owned(), "2026-10-20".to_owned(), true)
    );
    assert_eq!(
        days_to_read(last, Some(&watch("2026-09-20", "2026-10-20", true))),
        ("2026-08-23".to_owned(), "2026-09-19".to_owned(), false)
    );
    // Up to date: the last two weeks again.
    assert_eq!(
        days_to_read(last, Some(&watch("2026-08-23", "2026-11-19", false))),
        ("2026-11-07".to_owned(), last.to_owned(), false)
    );
    // Away for three weeks: from where it stopped; for months: a month.
    assert_eq!(
        days_to_read(last, Some(&watch("2026-08-01", "2026-10-30", false))),
        ("2026-10-30".to_owned(), last.to_owned(), false)
    );
    assert_eq!(
        days_to_read(last, Some(&watch("2026-01-01", "2026-06-01", false))),
        ("2026-10-21".to_owned(), last.to_owned(), false)
    );
    // A watch that only ever failed has read nothing.
    assert_eq!(
        days_to_read(last, Some(&AdsWatchState::default())),
        ("2026-10-21".to_owned(), last.to_owned(), true)
    );
}

/// A shop that just chose its account is read for its last month, four
/// questions for the lot, then for the months before, then once a day.
#[tokio::test]
async fn a_connected_shop_is_read_once_a_day() {
    let rig = Rig::new(3).await;
    assert_eq!(rig.only().sweep(rig.now).await.unwrap(), 3);
    rig.catch_up().await;

    let first = day_before(&rig.last, ADS_DAYS_A_PASS - 1);
    let asked = questions(&rig.stand_in);
    let reports: Vec<&str> = asked.iter().map(|(report, _, _)| report.as_str()).collect();
    assert_eq!(
        reports,
        vec![
            "campaign",
            "ad_group",
            "shopping_performance_view",
            "search_term_view"
        ]
    );
    assert!(
        asked
            .iter()
            .all(|(_, from, to)| *from == first && *to == rig.last)
    );
    assert_eq!(rig.stand_in.renewals(), 1);

    assert_eq!(
        last_ads_day(&rig.db, &rig.shop, ACCOUNT).await.unwrap(),
        Some(rig.last.clone())
    );
    let totals = ads_totals(&rig.db, &rig.shop, ACCOUNT, "2000-01-01", &rig.last)
        .await
        .unwrap();
    assert_eq!((totals.days, totals.currency.as_str()), (3, "EUR"));
    assert_eq!(totals.figures().cost_cents(), 3_600);
    assert_eq!(totals.figures().return_hundredths(), Some(333));
    let products = ads_by_product(&rig.db, &rig.shop, ACCOUNT, &rig.last, &rig.last, 10)
        .await
        .unwrap();
    assert_eq!(
        (products[0].key.as_str(), products[0].clicks),
        ("01arbre", 40)
    );
    let terms = ads_by_term(&rig.db, &rig.shop, ACCOUNT, &rig.last, &rig.last, 10)
        .await
        .unwrap();
    assert_eq!(terms[0].key, "arbre à chat");

    // Older days are owed: the next pass comes soon, and reads the month
    // before; the one after, what is left of the ninety days.
    let watch = rig.watch().await;
    assert!(watch.behind);
    assert_eq!(watch.calls, 4);
    assert_eq!(watch.next_at, watch.checked_at + CATCH_UP_SECS);
    assert_eq!(rig.only().sweep(watch.next_at - 1).await.unwrap(), 0);
    assert_eq!(rig.only().sweep(watch.next_at).await.unwrap(), 0);
    rig.catch_up().await;
    let watch = rig.watch().await;
    assert!(watch.behind);
    assert_eq!(rig.only().sweep(watch.next_at).await.unwrap(), 0);
    rig.catch_up().await;
    let asked = questions(&rig.stand_in);
    assert_eq!(asked.len(), 12);
    assert_eq!(asked[4].2, day_before(&first, 1));
    assert_eq!(asked[8].1, day_before(&rig.last, ADS_BACKFILL_DAYS - 1));
    let watch = rig.watch().await;
    assert!(!watch.behind);
    assert_eq!(
        watch.read_from,
        day_before(&rig.last, ADS_BACKFILL_DAYS - 1)
    );
    assert_eq!(watch.next_at, watch.checked_at + CHECK_EVERY_SECS);
    assert_eq!(rig.only().sweep(watch.next_at - 1).await.unwrap(), 0);

    // A day later: the last two weeks again. A conversion came for the
    // days already read, and one more day was shown: all four move.
    let next = watch.next_at;
    let tomorrow = last_finished_day(next, ZONE);
    {
        let mut held = rig.held.lock().unwrap();
        held.days.push(tomorrow.clone());
        held.value = 60.0;
    }
    assert_eq!(rig.only().sweep(next).await.unwrap(), 4);
    let asked = questions(&rig.stand_in);
    assert_eq!(asked.len(), 16);
    assert_eq!(
        (asked[12].1.clone(), asked[12].2.clone()),
        (day_before(&tomorrow, ADS_RECENT_DAYS - 1), tomorrow.clone())
    );
    rig.catch_up().await;
    let totals = ads_totals(&rig.db, &rig.shop, ACCOUNT, "2000-01-01", &tomorrow)
        .await
        .unwrap();
    assert_eq!(totals.days, 4);
    assert_eq!(totals.figures().value_cents(), 24_000);
    // The access held served every pass.
    assert_eq!(rig.stand_in.renewals(), 1);

    // Read again with nothing moved: nothing is recorded.
    let watch = rig.watch().await;
    assert_eq!(rig.only().sweep(watch.next_at).await.unwrap(), 0);
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
    assert_eq!(rig.days_held().await, 2);
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
    assert_eq!(rig.node(theirs, 2_880).sweep(rig.now).await.unwrap(), 0);
    assert!(questions(&rig.stand_in).is_empty());
    let mine = parts
        .into_iter()
        .find(|share| share.owns(&rig.shop))
        .unwrap();
    assert_eq!(rig.node(mine, 2_880).sweep(rig.now).await.unwrap(), 2);
}

/// The passes stop at their share of the day's allowance: the shop stays
/// due, and is read once there is room.
#[tokio::test]
async fn the_passes_stay_inside_the_days_allowance() {
    let rig = Rig::new(2).await;
    // Four fifths of four questions is not a pass.
    assert_eq!(
        rig.node(Share { index: 0, of: 1 }, 4)
            .sweep(rig.now)
            .await
            .unwrap(),
        0
    );
    assert!(questions(&rig.stand_in).is_empty());
    assert!(
        rig.command()
            .load_ads_watch(&rig.shop)
            .await
            .unwrap()
            .is_none()
    );

    // Room for one pass, not two: what the first asked is counted.
    let node = rig.node(Share { index: 0, of: 1 }, 9);
    assert_eq!(node.sweep(rig.now).await.unwrap(), 2);
    rig.catch_up().await;
    let watch = rig.watch().await;
    assert_eq!(ads_calls_on(&rig.db, &watch.calls_day).await.unwrap(), 4);
    // The tally is a day's (UTC): the next pass falls on the same one
    // unless the test runs across midnight.
    if format_date(watch.next_at) == watch.calls_day {
        assert_eq!(node.sweep(watch.next_at).await.unwrap(), 0);
        assert_eq!(questions(&rig.stand_in).len(), 4);
    }
    assert_eq!(rig.only().sweep(watch.next_at).await.unwrap(), 0);
    assert_eq!(questions(&rig.stand_in).len(), 8);
}

/// A pass Google does not answer is recorded with its reason and what it
/// asked, and tried again later, further apart each time.
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
    assert_eq!((watch.checked_at, watch.calls), (0, 1));
    assert_eq!(rig.only().sweep(watch.next_at - 1).await.unwrap(), 0);
    assert_eq!(questions(&rig.stand_in).len(), 1);
    assert!(watch.next_at >= rig.now + 3_600);

    // Google's own allowance used up half-way: nothing of the pass is
    // recorded, and the whole of it is asked again.
    {
        let mut held = rig.held.lock().unwrap();
        held.failing = None;
        held.fail_after = Some(held.asked + 2);
    }
    assert_eq!(rig.only().sweep(watch.next_at).await.unwrap(), 0);
    rig.catch_up().await;
    let watch = rig.watch().await;
    assert_eq!(watch.attempts, 2);
    assert_eq!(
        watch.last_error.as_deref(),
        Some("Google is unavailable: Too many requests (RESOURCE_EXHAUSTED)")
    );
    assert_eq!(watch.calls, 4);
    assert_eq!(rig.days_held().await, 0);

    rig.held.lock().unwrap().fail_after = None;
    assert_eq!(rig.only().sweep(watch.next_at).await.unwrap(), 3);
    let watch = rig.watch().await;
    assert_eq!(watch.attempts, 0);
    assert_eq!(watch.last_error, None);
    assert!(watch.checked_at > 0);
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
            .load_ads_watch(&rig.shop)
            .await
            .unwrap()
            .is_none()
    );
    rig.catch_up().await;
    assert!(ads_due(&rig.db, i64::MAX, "", 10).await.unwrap().is_empty());

    // Connected again, the account kept: read at the next sweep.
    rig.held.lock().unwrap().withdrawn = false;
    rig.connect("refresh-2").await;
    rig.catch_up().await;
    assert_eq!(rig.only().sweep(rig.now).await.unwrap(), 3);
}

/// A shop that took its connection off, or is suspended, is left alone.
#[tokio::test]
async fn a_disconnected_or_suspended_shop_is_left_alone() {
    let rig = Rig::new(2).await;
    timada_tenancy::Command(rig.store.clone())
        .suspend_tenant(&rig.shop, "root")
        .await
        .unwrap();
    assert_eq!(rig.only().sweep(rig.now).await.unwrap(), 0);
    assert!(questions(&rig.stand_in).is_empty());

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
    assert_eq!(questions(&rig.stand_in).len(), 4);
    assert_eq!(rig.days_held().await, 2);
}
