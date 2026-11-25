use evento::{Evento, Turso};
use timada_google::aggregator::{AdsCampaignRow, AdsNumbers, CampaignAdGroup};
use timada_google::{AdsDayReport, CreatedCampaign};
use timada_launch::{FIX, GO_ON};
use timada_tenancy::CreateTenant;
use topcoat::mail::{Mailbox, MemoryTransport};

use super::*;
use crate::google::tests::{StandIn, connected};
use crate::mailroom::tests::{BASE_URL, room, rows, store};

const ACCOUNT: &str = "1234567890";
const ALICE: &str = "user-alice";
const DAY: u64 = 86_400;

/// A shop whose owner has an address, connected to Google, its first
/// campaign live since now, which brought 85.00 for 100.00 over 640
/// clicks on its first day. The shop's id, and when the campaign started.
async fn shop(turso: &Turso) -> (String, u64) {
    let store = Evento::new(turso.clone());
    evento::append(ALICE)
        .original_version(0)
        .event(&timada_identity::aggregator::EmailRegistered {
            email: "alice@example.org".to_owned(),
            label: "Alice".to_owned(),
            password_hash: String::new(),
        })
        .commit(&store)
        .await
        .unwrap();
    let shop = timada_tenancy::Command(store.clone())
        .create_tenant(CreateTenant {
            name: "Acme Tea".to_owned(),
            owner_user_id: ALICE.to_owned(),
        })
        .await
        .unwrap()
        .id;
    timada_identity::read_model_subscription::<Turso>(Db::from(turso.clone()))
        .ephemeral()
        .run_once(turso)
        .await
        .unwrap();
    let google = StandIn::start(|_| (200, "{}".into())).await.google();
    connected(turso, &google, &shop, "refresh-1").await;
    let connections = timada_google::Command(store.clone());
    connections
        .choose_ads_account(
            ALICE,
            &shop,
            AdsAccount {
                customer_id: ACCOUNT.to_owned(),
                login_customer_id: None,
                name: "Acme".to_owned(),
                currency: "EUR".to_owned(),
                time_zone: "UTC".to_owned(),
            },
        )
        .await
        .unwrap();
    let settings = [
        ("name", "Shopping · Acme"),
        ("daily_budget", "10"),
        ("bid", "0.30"),
        ("countries", "FR"),
        ("start", "live"),
        ("ad_groups", "01CAT Arbres"),
    ];
    connections
        .record_campaign(
            ALICE,
            &shop,
            CreatedCampaign {
                kind: CAMPAIGN_SHOPPING.to_owned(),
                customer_id: ACCOUNT.to_owned(),
                campaign_id: "501".to_owned(),
                settings: settings
                    .into_iter()
                    .map(|(field, value)| (field.to_owned(), value.to_owned()))
                    .collect(),
                ad_groups: vec![CampaignAdGroup {
                    ad_group_id: "601".into(),
                    category_id: "01CAT".into(),
                    name: "Arbres".into(),
                }],
                proposal_id: "proposal-0".to_owned(),
            },
        )
        .await
        .unwrap();
    let started = held(&store, &shop, CAMPAIGN_SHOPPING)
        .await
        .unwrap()
        .unwrap()
        .live_at
        .unwrap();
    connections
        .record_ads_day(
            "reader",
            &shop,
            ACCOUNT,
            AdsDayReport {
                day: day_after(started, 0),
                currency: "EUR".into(),
                campaigns: vec![AdsCampaignRow {
                    campaign_id: "501".into(),
                    name: "Shopping · Acme".into(),
                    device: "mobile".into(),
                    numbers: AdsNumbers {
                        cost_micros: 100_000_000,
                        impressions: 20_000,
                        clicks: 640,
                        conversions_milli: 3_000,
                        value_micros: 85_000_000,
                    },
                }],
                ..AdsDayReport::default()
            },
        )
        .await
        .unwrap();
    timada_google::read_model_subscription::<Evento>(Db::from(turso.clone()))
        .run_once(&store)
        .await
        .unwrap();
    (shop, started)
}

async fn launch(store: &Evento, shop: &str) -> LaunchState {
    Launches::new(store.clone(), shop)
        .load_launch()
        .await
        .unwrap()
        .unwrap()
}

#[tokio::test]
async fn each_checkpoint_is_read_once_when_its_day_has_come() {
    let turso = store().await;
    let store = Evento::new(turso.clone());
    let db = Db::from(turso.clone());
    let (shop, started) = shop(&turso).await;
    let read = async |days: u64| {
        read_due(&store, &db, &shop, started + days * DAY)
            .await
            .unwrap()
    };

    assert_eq!(read(20).await, None);
    assert_eq!(read(21).await, Some(21));
    // Read by another node, or by tomorrow's pass: once.
    assert_eq!(read(21).await, None);
    assert_eq!(read(44).await, None);
    let reached = launch(&store, &shop).await.reached(21).cloned().unwrap();
    assert_eq!(reached.advice, GO_ON);
    assert_eq!(reached.started_at, started);
    assert_eq!(
        reached.figures,
        vec![
            (
                "Return on ad spend, as Google Ads counts it".to_owned(),
                "0.85".to_owned()
            ),
            ("Clicks".to_owned(), "640".to_owned()),
        ]
    );
    assert_eq!(
        reached.why,
        "A return of 0.85 is at or above 0.70: on course for a first campaign."
    );

    // No order came from an ad: nothing to hold against a break-even.
    assert_eq!(read(45).await, Some(45));
    let reached = launch(&store, &shop).await.reached(45).cloned().unwrap();
    assert_eq!(reached.advice, FIX);
    assert_eq!(
        reached.figures[2],
        ("Tuning round".to_owned(), "None yet".to_owned())
    );

    // A node that was away a long while reads what is owed, one a pass.
    assert_eq!(read(200).await, Some(90));
    assert_eq!(read(200).await, Some(120));
    assert_eq!(read(200).await, None);
    let launch = launch(&store, &shop).await;
    // Search Console is not connected: that is said, not judged.
    assert_eq!(launch.reached(90).unwrap().advice, FIX);
    assert_eq!(
        launch.reached(90).unwrap().figures,
        vec![("Search Console".to_owned(), "Not read".to_owned())]
    );
    assert_eq!(launch.reached(120).unwrap().advice, FIX);
}

#[tokio::test]
async fn a_shop_with_no_campaign_running_has_no_checkpoint() {
    let turso = store().await;
    let store = Evento::new(turso.clone());
    let db = Db::from(turso.clone());
    let shop = timada_tenancy::Command(store.clone())
        .create_tenant(CreateTenant {
            name: "Quiet".to_owned(),
            owner_user_id: ALICE.to_owned(),
        })
        .await
        .unwrap()
        .id;
    assert_eq!(
        read_due(&store, &db, &shop, crate::auth::now() + 400 * DAY)
            .await
            .unwrap(),
        None
    );
    let standing = standing(&store, &db, &shop, crate::auth::now())
        .await
        .unwrap();
    assert_eq!(standing, Standing::default());
}

#[tokio::test]
async fn the_owners_are_told_once_with_the_numbers() {
    let turso = store().await;
    let store = Evento::new(turso.clone());
    let db = Db::from(turso.clone());
    let transport = MemoryTransport::new();
    let node_a = room(&turso, transport.clone(), "node-a");
    let node_b = room(&turso, transport.clone(), "node-b");
    let (shop, started) = shop(&turso).await;
    read_due(&store, &db, &shop, started + 21 * DAY)
        .await
        .unwrap();
    let run = async |room: &Arc<Mailroom>| {
        notices_subscription::<Turso>(room.clone())
            .ephemeral()
            .no_retry()
            .run_once(&turso)
            .await
            .expect("a pass never fails for a mail");
        room.sweep(crate::auth::now()).await.unwrap()
    };
    assert_eq!(run(&node_a).await, 1);
    assert_eq!(run(&node_b).await, 0);
    let mails = transport.sent();
    assert_eq!(mails.len(), 1);
    assert_eq!(mails[0].to(), [Mailbox::new("alice@example.org").unwrap()]);
    assert_eq!(mails[0].subject(), "Day 21 of Acme Tea: Go on");
    let wire = String::from_utf8(mails[0].formatted())
        .unwrap()
        .replace("=\r\n", "");
    assert!(wire.contains("0.85"), "{wire}");
    assert!(wire.contains("Clicks: 640"), "{wire}");
    assert!(
        wire.contains(&checkpoints_link(BASE_URL, "acme-tea")),
        "{wire}"
    );
    run(&node_a).await;
    assert!(rows(&turso).await.is_empty());
    assert_eq!(transport.sent().len(), 1);
}

#[tokio::test]
async fn the_last_thirty_days_are_held_against_the_target() {
    let turso = store().await;
    let store = Evento::new(turso.clone());
    let db = Db::from(turso.clone());
    let (shop, started) = shop(&turso).await;
    let now = started + DAY;
    // Two orders that say what they left: 400.00 sold, 150.00 of profit.
    for (id, total, profit) in [("o1", 30_000, 12_000), ("o2", 10_000, 3_000)] {
        db.execute(
            "INSERT INTO orders_sales (tenant_id, order_id, placed_at, currency, counted,
                total_cents, refunded_cents, item_count, profit_cents, source)
             VALUES (?1, ?2, ?3, 'EUR', 1, ?4, 0, 1, ?5, NULL)",
            (shop.as_str(), id, started as i64, total, profit),
        )
        .await
        .unwrap();
    }
    let standing = standing(&store, &db, &shop, now).await.unwrap();
    assert_eq!(standing.started_at, Some(started));
    assert_eq!(standing.ahead, Some((21, started + 21 * DAY)));
    // 150.00 of profit less the 100.00 the ads cost.
    assert_eq!(standing.month_cents, Some(5_000));
    assert_eq!(standing.currency, "EUR");
    assert_eq!(standing.progress_percent(20_000), Some(25));
    assert_eq!(standing.progress_percent(0), None);
    // A month in the red is no progress, not a negative one.
    let red = Standing {
        month_cents: Some(-5_000),
        ..standing.clone()
    };
    assert_eq!(red.progress_percent(20_000), Some(0));
    // One sale that does not say what it left: the margin is not known.
    db.execute(
        "INSERT INTO orders_sales (tenant_id, order_id, placed_at, currency, counted,
            total_cents, refunded_cents, item_count, profit_cents, source)
         VALUES (?1, 'o3', ?2, 'EUR', 1, 5000, 0, 1, NULL, NULL)",
        (shop.as_str(), started as i64),
    )
    .await
    .unwrap();
    let unknown = super::standing(&store, &db, &shop, now).await.unwrap();
    assert_eq!(unknown.month_cents, None);
}
