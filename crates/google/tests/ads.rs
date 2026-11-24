mod common;

use common::*;
use timada_core::db::Db;
use timada_google::aggregator::{
    AdsAdGroupRow, AdsCampaignRow, AdsNumbers, AdsProductRow, AdsTermRow,
};
use timada_google::{
    ACCESS_ADS, ACCESS_SEARCH_CONSOLE, AdsAccount, AdsDayReport, CATCH_UP_SECS, CHECK_EVERY_SECS,
    Connect, DueAds, GOOGLE_ACTOR, GoogleError, KEPT_DAYS, KEPT_ROW_DAYS, MAX_ADS_PRODUCT_ROWS,
    ads_by_ad_group, ads_by_campaign, ads_by_device, ads_by_product, ads_by_term, ads_calls_on,
    ads_due, ads_totals, day_before, last_ads_day, read_model_subscription,
};

const ACCOUNT: &str = "1234567890";
const OTHER: &str = "9876543210";

async fn catch_up(executor: &evento::Turso, db: &Db) {
    read_model_subscription::<evento::Turso>(db.clone())
        .run_once(executor)
        .await
        .unwrap();
}

fn account(customer_id: &str) -> AdsAccount {
    AdsAccount {
        customer_id: customer_id.to_owned(),
        login_customer_id: None,
        name: "Acme".to_owned(),
        currency: "eur".to_owned(),
        time_zone: "europe/paris".to_owned(),
    }
}

/// What `account` granted: Search Console and Google Ads, under `token`.
fn wide(account: &str, token: &[u8]) -> Connect {
    Connect {
        refresh_token_sealed: token.to_vec(),
        account: account.to_owned(),
        accesses: vec![ACCESS_ADS.to_owned(), ACCESS_SEARCH_CONSOLE.to_owned()],
    }
}

fn numbers(cost_micros: u64, clicks: u64, value_micros: u64) -> AdsNumbers {
    AdsNumbers {
        cost_micros,
        impressions: clicks * 50,
        clicks,
        conversions_milli: value_micros / 20_000,
        value_micros,
    }
}

/// A day of one Shopping campaign on phones and computers: 12.00 spent for
/// 40 clicks and 40.00 of conversions, over two ad groups, two products and
/// two searches.
fn day(day: &str) -> AdsDayReport {
    AdsDayReport {
        day: day.to_owned(),
        currency: "EUR".to_owned(),
        campaigns: vec![
            AdsCampaignRow {
                campaign_id: "11".to_owned(),
                name: "Shopping".to_owned(),
                device: "mobile".to_owned(),
                numbers: numbers(9_000_000, 30, 20_000_000),
            },
            AdsCampaignRow {
                campaign_id: "11".to_owned(),
                name: "Shopping".to_owned(),
                device: "desktop".to_owned(),
                numbers: numbers(3_000_000, 10, 20_000_000),
            },
        ],
        ad_groups: vec![
            AdsAdGroupRow {
                campaign_id: "11".to_owned(),
                ad_group_id: "21".to_owned(),
                name: "Arbres à chat".to_owned(),
                numbers: numbers(8_000_000, 25, 40_000_000),
            },
            AdsAdGroupRow {
                campaign_id: "11".to_owned(),
                ad_group_id: "22".to_owned(),
                name: "Griffoirs".to_owned(),
                numbers: numbers(4_000_000, 15, 0),
            },
        ],
        products: vec![
            AdsProductRow {
                item_id: "01ARBRE".to_owned(),
                campaign_id: "11".to_owned(),
                ad_group_id: "21".to_owned(),
                numbers: numbers(8_000_000, 25, 40_000_000),
            },
            AdsProductRow {
                item_id: "01griffoir".to_owned(),
                campaign_id: "11".to_owned(),
                ad_group_id: "22".to_owned(),
                numbers: numbers(4_000_000, 15, 0),
            },
        ],
        terms: vec![
            AdsTermRow {
                term: "arbre à chat".to_owned(),
                campaign_id: "11".to_owned(),
                ad_group_id: "21".to_owned(),
                numbers: numbers(6_000_000, 20, 40_000_000),
            },
            AdsTermRow {
                term: "griffoir carton".to_owned(),
                campaign_id: "11".to_owned(),
                ad_group_id: "22".to_owned(),
                numbers: numbers(4_000_000, 15, 0),
            },
        ],
    }
}

#[tokio::test]
async fn a_shop_chooses_its_ads_account_among_those_the_grant_reaches() {
    let google = command().await;
    assert!(matches!(
        google
            .choose_ads_account(ALICE, SHOP, account(ACCOUNT))
            .await,
        Err(GoogleError::NotConnected)
    ));
    // Search Console alone does not reach Google Ads.
    google
        .connect(ALICE, SHOP, grant("alice@example.com", b"sealed-1"))
        .await
        .unwrap();
    assert!(matches!(
        google
            .choose_ads_account(ALICE, SHOP, account(ACCOUNT))
            .await,
        Err(GoogleError::NoAds)
    ));
    google
        .connect(ALICE, SHOP, wide("alice@example.com", b"sealed-2"))
        .await
        .unwrap();
    for bad in ["", "12345", "customers/1234567890"] {
        assert!(
            matches!(
                google.choose_ads_account(ALICE, SHOP, account(bad)).await,
                Err(GoogleError::InvalidAdsAccount)
            ),
            "{bad}"
        );
    }
    let mut yen = account(ACCOUNT);
    yen.currency = "yens".to_owned();
    assert!(matches!(
        google.choose_ads_account(ALICE, SHOP, yen).await,
        Err(GoogleError::InvalidAdsAccount)
    ));

    // As the pages of Google Ads write it, through a manager.
    let mut chosen = account("123-456-7890");
    chosen.login_customer_id = Some(OTHER.to_owned());
    google
        .choose_ads_account(ALICE, SHOP, chosen.clone())
        .await
        .unwrap();
    google
        .choose_ads_account(ALICE, SHOP, chosen)
        .await
        .unwrap();
    let connection = google.load_connection(SHOP).await.unwrap().unwrap();
    assert_eq!(connection.aggregate_version, 3);
    assert_eq!(
        connection.ads,
        Some(AdsAccount {
            customer_id: ACCOUNT.to_owned(),
            login_customer_id: Some(OTHER.to_owned()),
            name: "Acme".to_owned(),
            currency: "EUR".to_owned(),
            time_zone: "Europe/Paris".to_owned(),
        })
    );

    // The same account connecting again keeps it; a grant without Google
    // Ads, or another account's, does not.
    google
        .connect(ALICE, SHOP, wide("alice@example.com", b"sealed-3"))
        .await
        .unwrap();
    let connection = google.load_connection(SHOP).await.unwrap().unwrap();
    assert!(connection.ads.is_some());
    google
        .connect(ALICE, SHOP, wide("bob@example.com", b"sealed-4"))
        .await
        .unwrap();
    let connection = google.load_connection(SHOP).await.unwrap().unwrap();
    assert_eq!(connection.ads, None);
    google
        .choose_ads_account(ALICE, SHOP, account(ACCOUNT))
        .await
        .unwrap();
    google
        .connect(ALICE, SHOP, grant("bob@example.com", b"sealed-5"))
        .await
        .unwrap();
    let connection = google.load_connection(SHOP).await.unwrap().unwrap();
    assert_eq!(connection.ads, None);

    // A lapsed grant chooses nothing.
    google
        .connect(ALICE, SHOP, wide("bob@example.com", b"sealed-6"))
        .await
        .unwrap();
    assert!(
        google
            .note_lapsed(GOOGLE_ACTOR, SHOP, b"sealed-6")
            .await
            .unwrap()
    );
    assert!(matches!(
        google
            .choose_ads_account(ALICE, SHOP, account(ACCOUNT))
            .await,
        Err(GoogleError::Lapsed)
    ));
}

#[tokio::test]
async fn a_day_is_recorded_again_only_when_its_numbers_moved() {
    let (executor, db) = executor_with_pool().await;
    let google = timada_google::Command(executor.clone());
    let record = |report: AdsDayReport| {
        let google = &google;
        async move {
            google
                .record_ads_day(GOOGLE_ACTOR, SHOP, ACCOUNT, report)
                .await
        }
    };
    assert!(record(day("2026-11-20")).await.unwrap());
    // Read the same the next day, whatever order Google lists the rows in.
    let mut same = day("2026-11-20");
    same.campaigns.reverse();
    same.products.reverse();
    same.terms.reverse();
    assert!(!record(same).await.unwrap());

    catch_up(&executor, &db).await;
    let totals = ads_totals(&db, SHOP, ACCOUNT, "2026-11-01", "2026-11-30")
        .await
        .unwrap();
    assert_eq!((totals.days, totals.currency.as_str()), (1, "EUR"));
    let figures = totals.figures();
    assert_eq!(figures.cost_cents(), 1_200);
    assert_eq!(figures.clicks, 40);
    assert_eq!(figures.value_cents(), 4_000);
    assert_eq!(figures.return_hundredths(), Some(333));
    assert_eq!(figures.click_cost_cents(), Some(30));
    assert_eq!(figures.click_rate_bps(), Some(200));

    // A conversion came later: the day is recorded again, and replaces
    // what it read before.
    let mut later = day("2026-11-20");
    later.campaigns[0].numbers.value_micros += 20_000_000;
    later.campaigns[0].numbers.conversions_milli += 1_000;
    later.terms.pop();
    assert!(record(later).await.unwrap());
    catch_up(&executor, &db).await;
    let totals = ads_totals(&db, SHOP, ACCOUNT, "2026-11-20", "2026-11-20")
        .await
        .unwrap();
    assert_eq!(totals.figures().value_cents(), 6_000);
    assert_eq!(totals.figures().return_hundredths(), Some(500));
    let terms = ads_by_term(&db, SHOP, ACCOUNT, "2026-11-20", "2026-11-20", 10)
        .await
        .unwrap();
    assert_eq!(terms.len(), 1);
    assert_eq!(terms[0].key, "arbre à chat");

    // Another day, and the period adds them up, by each thing.
    assert!(record(day("2026-11-21")).await.unwrap());
    catch_up(&executor, &db).await;
    assert_eq!(
        last_ads_day(&db, SHOP, ACCOUNT).await.unwrap().as_deref(),
        Some("2026-11-21")
    );
    assert_eq!(last_ads_day(&db, SHOP, OTHER).await.unwrap(), None);
    let period = ("2026-11-01", "2026-11-30");
    let campaigns = ads_by_campaign(&db, SHOP, ACCOUNT, period.0, period.1)
        .await
        .unwrap();
    assert_eq!(campaigns.len(), 1);
    assert_eq!(
        (campaigns[0].key.as_str(), campaigns[0].name.as_str()),
        ("11", "Shopping")
    );
    assert_eq!(campaigns[0].figures().cost_cents(), 2_400);
    let devices = ads_by_device(&db, SHOP, ACCOUNT, period.0, period.1)
        .await
        .unwrap();
    let read: Vec<_> = devices
        .iter()
        .map(|line| (line.key.as_str(), line.clicks))
        .collect();
    assert_eq!(read, vec![("mobile", 60), ("desktop", 20)]);
    let groups = ads_by_ad_group(&db, SHOP, ACCOUNT, period.0, period.1)
        .await
        .unwrap();
    assert_eq!(groups[0].name, "Arbres à chat");
    assert_eq!(groups[1].figures().return_hundredths(), Some(0));
    // An item's id reads in lower case, as Google Ads reports it.
    let products = ads_by_product(&db, SHOP, ACCOUNT, period.0, period.1, 1)
        .await
        .unwrap();
    assert_eq!(products.len(), 1);
    assert_eq!(products[0].key, "01arbre");
    assert_eq!(products[0].clicks, 50);

    // Another account's days, and another shop's, are their own.
    assert!(
        google
            .record_ads_day(GOOGLE_ACTOR, SHOP, OTHER, day("2026-11-20"))
            .await
            .unwrap()
    );
    assert!(
        google
            .record_ads_day(GOOGLE_ACTOR, "tenant-other", ACCOUNT, day("2026-11-20"))
            .await
            .unwrap()
    );
    for bad in ["2026-11-31", "20 November", ""] {
        assert!(matches!(
            record(day(bad)).await,
            Err(GoogleError::InvalidDay)
        ));
    }
}

#[tokio::test]
async fn a_day_keeps_the_rows_that_cost_the_most_and_say_something() {
    let (executor, db) = executor_with_pool().await;
    let google = timada_google::Command(executor.clone());
    let mut report = day("2026-11-20");
    // Shown nowhere, on no device Google Ads knows, under no id of its own.
    report.campaigns.push(AdsCampaignRow {
        campaign_id: "12".to_owned(),
        name: "Paused".to_owned(),
        device: "mobile".to_owned(),
        numbers: AdsNumbers::default(),
    });
    report.campaigns.push(AdsCampaignRow {
        campaign_id: "13".to_owned(),
        name: "Fridge".to_owned(),
        device: "fridge".to_owned(),
        numbers: numbers(1_000_000, 1, 0),
    });
    report.ad_groups.push(AdsAdGroupRow {
        campaign_id: "11".to_owned(),
        ad_group_id: "customers/1/adGroups/2".to_owned(),
        name: "Path".to_owned(),
        numbers: numbers(1_000_000, 1, 0),
    });
    report.products = (0..MAX_ADS_PRODUCT_ROWS + 10)
        .map(|n| AdsProductRow {
            item_id: format!("item-{n:05}"),
            campaign_id: "11".to_owned(),
            ad_group_id: "21".to_owned(),
            numbers: numbers(1_000 * (n as u64 + 1), 1, 0),
        })
        .collect();
    google
        .record_ads_day(GOOGLE_ACTOR, SHOP, ACCOUNT, report)
        .await
        .unwrap();
    catch_up(&executor, &db).await;
    let day = ("2026-11-20", "2026-11-20");
    assert_eq!(
        ads_by_campaign(&db, SHOP, ACCOUNT, day.0, day.1)
            .await
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        ads_by_ad_group(&db, SHOP, ACCOUNT, day.0, day.1)
            .await
            .unwrap()
            .len(),
        2
    );
    let products = ads_by_product(&db, SHOP, ACCOUNT, day.0, day.1, 10_000)
        .await
        .unwrap();
    assert_eq!(products.len(), MAX_ADS_PRODUCT_ROWS);
    // The ten that cost the least were left out.
    assert_eq!(products.last().unwrap().key, "item-00010");
}

#[tokio::test]
async fn the_history_kept_is_bounded() {
    let (executor, db) = executor_with_pool().await;
    let google = timada_google::Command(executor.clone());
    let newest = "2026-11-20";
    for back in [
        0,
        KEPT_ROW_DAYS,
        KEPT_ROW_DAYS + 1,
        KEPT_DAYS,
        KEPT_DAYS + 1,
    ] {
        google
            .record_ads_day(GOOGLE_ACTOR, SHOP, ACCOUNT, day(&day_before(newest, back)))
            .await
            .unwrap();
    }
    // The newest is read last: what is older than the history goes with it.
    google
        .record_ads_day(GOOGLE_ACTOR, SHOP, ACCOUNT, {
            let mut again = day(newest);
            again.campaigns[0].numbers.clicks += 1;
            again
        })
        .await
        .unwrap();
    catch_up(&executor, &db).await;
    let totals = ads_totals(&db, SHOP, ACCOUNT, "2000-01-01", newest)
        .await
        .unwrap();
    assert_eq!(totals.days, 4);
    let products = ads_by_product(&db, SHOP, ACCOUNT, "2000-01-01", newest, 10)
        .await
        .unwrap();
    // Two days of rows: the newest and the one at the edge.
    assert_eq!(products[0].clicks, 50);
}

#[tokio::test]
async fn a_pass_says_when_the_next_one_is_due_and_what_it_asked() {
    let (executor, db) = executor_with_pool().await;
    let google = timada_google::Command(executor.clone());
    assert!(google.load_ads_watch(SHOP).await.unwrap().is_none());
    google
        .record_ads_check(
            GOOGLE_ACTOR,
            SHOP,
            ACCOUNT,
            "2026-10-21",
            "2026-11-20",
            true,
            4,
        )
        .await
        .unwrap();
    let watch = google.load_ads_watch(SHOP).await.unwrap().unwrap();
    assert!(watch.behind);
    assert_eq!(watch.read_from, "2026-10-21");
    assert_eq!(watch.read_to, "2026-11-20");
    assert_eq!(watch.next_at, watch.checked_at + CATCH_UP_SECS);
    assert!(!watch.is_due(ACCOUNT, watch.next_at - 1));
    assert!(watch.is_due(ACCOUNT, watch.next_at));
    assert!(watch.is_due(OTHER, watch.checked_at));

    // Older days read: how far back the account is read only grows.
    google
        .record_ads_check(
            GOOGLE_ACTOR,
            SHOP,
            ACCOUNT,
            "2026-09-20",
            "2026-10-20",
            false,
            4,
        )
        .await
        .unwrap();
    google
        .record_ads_check(
            GOOGLE_ACTOR,
            SHOP,
            ACCOUNT,
            "2026-11-07",
            "2026-11-20",
            false,
            4,
        )
        .await
        .unwrap();
    let watch = google.load_ads_watch(SHOP).await.unwrap().unwrap();
    assert!(!watch.behind);
    assert_eq!(watch.read_from, "2026-09-20");
    assert_eq!(watch.next_at, watch.checked_at + CHECK_EVERY_SECS);
    let checked_at = watch.checked_at;

    google
        .record_ads_failure(
            GOOGLE_ACTOR,
            SHOP,
            ACCOUNT,
            " Google is unavailable: 503 ",
            1,
        )
        .await
        .unwrap();
    google
        .record_ads_failure(GOOGLE_ACTOR, SHOP, ACCOUNT, "again", 1)
        .await
        .unwrap();
    let watch = google.load_ads_watch(SHOP).await.unwrap().unwrap();
    assert_eq!(watch.attempts, 2);
    assert_eq!(watch.last_error.as_deref(), Some("again"));
    assert_eq!(watch.checked_at, checked_at);
    assert_eq!(watch.read_from, "2026-09-20");
    // Every question counts in the day's tally, the failed ones too.
    assert_eq!(watch.calls, 14);
    catch_up(&executor, &db).await;
    assert_eq!(ads_calls_on(&db, &watch.calls_day).await.unwrap(), 14);
    assert_eq!(ads_calls_on(&db, "2000-01-01").await.unwrap(), 0);

    // Another shop's passes count in the same day.
    google
        .record_ads_check(
            GOOGLE_ACTOR,
            "tenant-other",
            ACCOUNT,
            "2026-11-07",
            "2026-11-20",
            false,
            4,
        )
        .await
        .unwrap();
    catch_up(&executor, &db).await;
    catch_up(&executor, &db).await;
    assert_eq!(ads_calls_on(&db, &watch.calls_day).await.unwrap(), 18);

    // A failure on another account starts from nothing.
    google
        .record_ads_failure(GOOGLE_ACTOR, SHOP, OTHER, "refused", 1)
        .await
        .unwrap();
    let watch = google.load_ads_watch(SHOP).await.unwrap().unwrap();
    assert_eq!(watch.customer_id, OTHER);
    assert_eq!(watch.attempts, 1);
    assert_eq!(watch.checked_at, 0);
    assert_eq!(watch.read_from, "");
    assert!(matches!(
        google
            .record_ads_check(
                GOOGLE_ACTOR,
                SHOP,
                ACCOUNT,
                "2026-11-21",
                "2026-11-20",
                false,
                1
            )
            .await,
        Err(GoogleError::InvalidDay)
    ));
}

#[tokio::test]
async fn the_shops_due_are_the_connected_ones_with_an_account_and_no_pass_ahead() {
    let (executor, db) = executor_with_pool().await;
    let google = timada_google::Command(executor.clone());
    let due = |now: i64| {
        let db = db.clone();
        async move { ads_due(&db, now, "", 100).await.unwrap() }
    };
    let shop = |customer_id: &str| DueAds {
        tenant_id: SHOP.to_owned(),
        customer_id: customer_id.to_owned(),
    };
    google
        .connect(ALICE, SHOP, wide("alice@example.com", b"sealed-1"))
        .await
        .unwrap();
    catch_up(&executor, &db).await;
    assert!(due(i64::MAX).await.is_empty());

    google
        .choose_ads_account(ALICE, SHOP, account(ACCOUNT))
        .await
        .unwrap();
    catch_up(&executor, &db).await;
    assert_eq!(due(0).await, vec![shop(ACCOUNT)]);

    google
        .record_ads_check(
            GOOGLE_ACTOR,
            SHOP,
            ACCOUNT,
            "2026-11-07",
            "2026-11-20",
            false,
            4,
        )
        .await
        .unwrap();
    catch_up(&executor, &db).await;
    let watch = google.load_ads_watch(SHOP).await.unwrap().unwrap();
    assert!(due(watch.next_at as i64 - 1).await.is_empty());
    assert_eq!(due(watch.next_at as i64).await, vec![shop(ACCOUNT)]);

    // Another account: due at once.
    google
        .choose_ads_account(ALICE, SHOP, account(OTHER))
        .await
        .unwrap();
    catch_up(&executor, &db).await;
    assert_eq!(due(0).await, vec![shop(OTHER)]);

    // A lapsed grant is left alone until the member connects again.
    google
        .note_lapsed(GOOGLE_ACTOR, SHOP, b"sealed-1")
        .await
        .unwrap();
    catch_up(&executor, &db).await;
    assert!(due(i64::MAX).await.is_empty());
    google
        .connect(ALICE, SHOP, wide("alice@example.com", b"sealed-2"))
        .await
        .unwrap();
    catch_up(&executor, &db).await;
    assert_eq!(due(0).await, vec![shop(OTHER)]);

    google.disconnect(ALICE, SHOP).await.unwrap();
    catch_up(&executor, &db).await;
    assert!(due(i64::MAX).await.is_empty());
}
