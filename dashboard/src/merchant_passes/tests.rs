use std::sync::Mutex;

use evento::Evento;
use timada_google::{
    CHECK_EVERY_SECS, MERCHANT_REFUSED, MerchantAccount, MerchantWatchState, merchant_problems,
    merchant_standing,
};
use timada_tenancy::CreateTenant;

use super::*;
use crate::google::tests::{Request, StandIn, connected, token_answer};

const ACCOUNT: &str = "5550001";

/// What the stand-in's Merchant Center holds and how it answers.
#[derive(Default)]
struct Held {
    /// The items Google refuses for Shopping ads; the others it accepts.
    refused: Vec<&'static str>,
    /// The status every question gets, when not 200.
    failing: Option<u16>,
    /// The grant is withdrawn: no access is given any more.
    withdrawn: bool,
}

const ITEMS: [&str; 3] = ["01TREE", "01POST", "01BOWL"];

fn answer(held: &Mutex<Held>, request: &Request) -> (u16, String) {
    let held = held.lock().unwrap();
    if request.line.starts_with("POST /token") {
        return if held.withdrawn {
            (400, r#"{"error":"invalid_grant"}"#.into())
        } else {
            token_answer("access-1")
        };
    }
    assert_eq!(request.authorization, "Bearer access-1");
    if let Some(status) = held.failing {
        return (status, r#"{"error":{"message":"Backend Error"}}"#.into());
    }
    if request.line.contains("/issues") {
        return (200, "{}".into());
    }
    assert!(
        request
            .line
            .starts_with("GET /merchant/products/v1/accounts/5550001/products?"),
        "{}",
        request.line
    );
    let products: Vec<serde_json::Value> = ITEMS
        .iter()
        .map(|item| {
            let ads = if held.refused.contains(item) {
                serde_json::json!({"reportingContext": "SHOPPING_ADS", "disapprovedCountries": ["FR"]})
            } else {
                serde_json::json!({"reportingContext": "SHOPPING_ADS", "approvedCountries": ["FR"]})
            };
            serde_json::json!({
                "offerId": item,
                "productStatus": {"destinationStatuses": [
                    ads,
                    {"reportingContext": "FREE_LISTINGS", "approvedCountries": ["FR"]},
                ]},
            })
        })
        .collect();
    (200, serde_json::json!({ "products": products }).to_string())
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
}

impl Rig {
    /// A shop connected to Google, its Merchant Center account chosen.
    async fn new() -> Self {
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
        let held = Arc::new(Mutex::new(Held::default()));
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
            now: crate::auth::now(),
        };
        rig.connect("refresh-1").await;
        rig.command()
            .choose_merchant_account(
                "alice",
                &rig.shop,
                MerchantAccount {
                    account_id: ACCOUNT.to_owned(),
                    name: "Acme".to_owned(),
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

    fn node(&self, share: Share) -> Arc<MerchantPasses> {
        MerchantPasses::new(
            self.db.clone(),
            self.store.clone(),
            Arc::new(move || share),
            self.google.clone(),
        )
    }

    fn only(&self) -> Arc<MerchantPasses> {
        self.node(Share { index: 0, of: 1 })
    }

    /// The read model the sweep reads, brought up to date.
    async fn catch_up(&self) {
        timada_google::read_model_subscription::<Evento>(self.db.clone())
            .run_once(&self.store)
            .await
            .unwrap();
    }

    async fn watch(&self) -> MerchantWatchState {
        self.command()
            .load_merchant_watch(&self.shop)
            .await
            .unwrap()
            .unwrap()
    }

    /// How many questions about the products and the account were asked.
    fn asked(&self) -> usize {
        self.stand_in
            .lines()
            .iter()
            .filter(|line| line.contains("/merchant/"))
            .count()
    }
}

/// A shop that just chose its account is read at once, then once a day,
/// and a read that says the same is recorded once.
#[tokio::test]
async fn a_connected_shop_is_read_once_a_day() {
    let rig = Rig::new().await;
    assert_eq!(rig.only().sweep(rig.now).await.unwrap(), 1);
    rig.catch_up().await;
    // The products, then the account.
    assert_eq!(rig.asked(), 2);
    let standing = merchant_standing(&rig.db, &rig.shop, ACCOUNT)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(standing.products(), 3);
    assert!(standing.all_accepted_for_ads());
    let watch = rig.watch().await;
    assert_eq!(watch.products, 3);
    assert_eq!(watch.next_at, watch.checked_at + CHECK_EVERY_SECS);
    assert_eq!(rig.only().sweep(watch.next_at - 1).await.unwrap(), 0);
    assert_eq!(rig.asked(), 2);

    // A day later Google refuses one: the read says what turned.
    rig.held.lock().unwrap().refused = vec!["01POST"];
    assert_eq!(rig.only().sweep(watch.next_at).await.unwrap(), 1);
    rig.catch_up().await;
    let feed = rig
        .command()
        .load_merchant_feed(&rig.shop, ACCOUNT)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(feed.aggregate_version, 2);
    let problems = merchant_problems(&rig.db, &rig.shop, ACCOUNT, 10)
        .await
        .unwrap();
    assert_eq!(problems.len(), 1);
    assert_eq!(
        (problems[0].item_id.as_str(), problems[0].ads.as_str()),
        ("01POST", MERCHANT_REFUSED)
    );

    // The next day it says the same: the feed is left as it is, and the
    // pass still counts.
    let watch = rig.watch().await;
    assert_eq!(rig.only().sweep(watch.next_at).await.unwrap(), 1);
    let feed = rig
        .command()
        .load_merchant_feed(&rig.shop, ACCOUNT)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(feed.aggregate_version, 2);
    assert_eq!(rig.watch().await.aggregate_version, 3);
    // The access held served every pass.
    assert_eq!(rig.stand_in.renewals(), 1);
}

/// A member who asks is read for whatever the watch says of when.
#[tokio::test]
async fn a_read_asked_for_does_not_wait_for_the_day() {
    let rig = Rig::new().await;
    assert_eq!(rig.only().sweep(rig.now).await.unwrap(), 1);
    rig.held.lock().unwrap().refused = vec!["01TREE"];
    assert_eq!(rig.only().sweep(rig.now).await.unwrap(), 0);
    assert_eq!(
        read_shop(&rig.google, &rig.store, &rig.shop).await.unwrap(),
        Read::Done {
            products: 3,
            moved: true
        }
    );
    assert_eq!(
        read_shop(&rig.google, &rig.store, &rig.shop).await.unwrap(),
        Read::Done {
            products: 3,
            moved: false
        }
    );
    assert_eq!(
        read_shop(&rig.google, &rig.store, "tenant-none")
            .await
            .unwrap(),
        Read::Nothing
    );
}

/// Two nodes that both think a shop is theirs record its read once; a node
/// leaves the other nodes' shops alone.
#[tokio::test]
async fn two_nodes_record_a_read_once_and_each_reads_its_part() {
    let rig = Rig::new().await;
    let parts = [Share { index: 0, of: 2 }, Share { index: 1, of: 2 }];
    let theirs = parts
        .into_iter()
        .find(|share| !share.owns(&rig.shop))
        .unwrap();
    assert_eq!(rig.node(theirs).sweep(rig.now).await.unwrap(), 0);
    assert_eq!(rig.asked(), 0);

    let (a, b) = (rig.only(), rig.only());
    let (first, second) = tokio::join!(a.sweep(rig.now), b.sweep(rig.now));
    assert_eq!(first.unwrap() + second.unwrap(), 2);
    let feed = rig
        .command()
        .load_merchant_feed(&rig.shop, ACCOUNT)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(feed.aggregate_version, 1);
}

/// A pass Google does not answer is recorded with its reason and tried
/// again later; a withdrawn grant is recorded on the connection instead.
#[tokio::test]
async fn a_pass_that_fails_is_recorded_and_tried_again() {
    let rig = Rig::new().await;
    rig.held.lock().unwrap().failing = Some(500);
    assert_eq!(rig.only().sweep(rig.now).await.unwrap(), 0);
    rig.catch_up().await;
    let watch = rig.watch().await;
    assert_eq!(watch.attempts, 1);
    assert_eq!(
        watch.last_error.as_deref(),
        Some("Google is unavailable: Backend Error")
    );
    assert_eq!(rig.only().sweep(watch.next_at - 1).await.unwrap(), 0);
    assert!(watch.next_at >= rig.now + 3_600);
    rig.held.lock().unwrap().failing = None;
    assert_eq!(rig.only().sweep(watch.next_at).await.unwrap(), 1);
    rig.catch_up().await;
    let watch = rig.watch().await;
    assert_eq!((watch.attempts, watch.last_error.clone()), (0, None));

    rig.held.lock().unwrap().withdrawn = true;
    // The access this node holds is still good: Google turns it down.
    rig.connect("refresh-2").await;
    rig.catch_up().await;
    assert_eq!(rig.only().sweep(watch.next_at).await.unwrap(), 0);
    let connection = rig
        .command()
        .load_connection(&rig.shop)
        .await
        .unwrap()
        .unwrap();
    assert!(connection.lapsed);
    // No failure of the pass: the connection says it.
    assert_eq!(rig.watch().await.attempts, 0);
    rig.catch_up().await;
    assert!(
        merchant_due(&rig.db, i64::MAX, "", 10)
            .await
            .unwrap()
            .is_empty()
    );
}

/// A suspended shop is not read for.
#[tokio::test]
async fn a_suspended_shop_is_left_alone() {
    let rig = Rig::new().await;
    timada_tenancy::Command(rig.store.clone())
        .suspend_tenant(&rig.shop, "root")
        .await
        .unwrap();
    assert_eq!(rig.only().sweep(rig.now).await.unwrap(), 0);
    assert_eq!(rig.asked(), 0);
}
