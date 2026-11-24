mod common;

use common::*;
use timada_core::db::Db;
use timada_google::{
    ACCESS_ADS, CATCH_UP_SECS, CHECK_EVERY_SECS, Connect, DueSearch, GOOGLE_ACTOR, GoogleError,
    KEPT_DAYS, KEPT_ROW_DAYS, MAX_SEARCH_ROWS, day_before, last_search_day,
    read_model_subscription, search_days_held, search_totals, searches_due, top_pages, top_queries,
};

const SITE: &str = "sc-domain:shop.example";
const OTHER_SITE: &str = "https://blog.shop.example/";
const FAR: i64 = i64::MAX;

async fn catch_up(executor: &evento::Turso, db: &Db) {
    read_model_subscription::<evento::Turso>(db.clone())
        .run_once(executor)
        .await
        .unwrap();
}

/// The shops due at `now`, by id.
async fn due(db: &Db, now: i64) -> Vec<DueSearch> {
    searches_due(db, now, "", 100).await.unwrap()
}

fn shop(tenant_id: &str, site: &str) -> DueSearch {
    DueSearch {
        tenant_id: tenant_id.to_owned(),
        site: site.to_owned(),
    }
}

#[tokio::test]
async fn a_day_is_recorded_once_whoever_fetched_it() {
    let google = command().await;
    assert!(
        google
            .record_search_day(GOOGLE_ACTOR, SHOP, SITE, day("2026-11-20"))
            .await
            .unwrap()
    );
    // Another node fetched the same day: the first one stands.
    let mut again = day("2026-11-20");
    again.numbers.clicks = 99;
    assert!(
        !google
            .record_search_day(GOOGLE_ACTOR, SHOP, SITE, again)
            .await
            .unwrap()
    );
    // Another site's same day, and another shop's, are days of their own.
    assert!(
        google
            .record_search_day(GOOGLE_ACTOR, SHOP, OTHER_SITE, day("2026-11-20"))
            .await
            .unwrap()
    );
    assert!(
        google
            .record_search_day(GOOGLE_ACTOR, "tenant-other", SITE, day("2026-11-20"))
            .await
            .unwrap()
    );
    for bad in ["2026-11-31", "20 November", "2026-11-2", ""] {
        assert!(
            matches!(
                google
                    .record_search_day(GOOGLE_ACTOR, SHOP, SITE, day(bad))
                    .await,
                Err(GoogleError::InvalidDay)
            ),
            "{bad}"
        );
    }
    assert!(matches!(
        google
            .record_search_day(GOOGLE_ACTOR, SHOP, "shop.example", day("2026-11-21"))
            .await,
        Err(GoogleError::InvalidSite)
    ));
}

#[tokio::test]
async fn a_pass_says_when_the_next_one_is_due() {
    let google = command().await;
    assert!(google.load_search_watch(SHOP).await.unwrap().is_none());

    // Older days left: the next pass comes soon.
    google
        .record_search_check(GOOGLE_ACTOR, SHOP, SITE, true)
        .await
        .unwrap();
    let watch = google.load_search_watch(SHOP).await.unwrap().unwrap();
    assert_eq!(watch.tenant_id, SHOP);
    assert!(watch.behind);
    assert_eq!(watch.next_at, watch.checked_at + CATCH_UP_SECS);
    assert!(!watch.is_due(SITE, watch.next_at - 1));
    assert!(watch.is_due(SITE, watch.next_at));
    // Another site was never looked at.
    assert!(watch.is_due(OTHER_SITE, watch.checked_at));

    google
        .record_search_check(GOOGLE_ACTOR, SHOP, SITE, false)
        .await
        .unwrap();
    let watch = google.load_search_watch(SHOP).await.unwrap().unwrap();
    assert!(!watch.behind);
    assert_eq!(watch.next_at, watch.checked_at + CHECK_EVERY_SECS);
    let checked_at = watch.checked_at;

    // Failures in a row wait an hour, then two; what was read stays read.
    google
        .record_search_failure(GOOGLE_ACTOR, SHOP, SITE, "  Google is unavailable: 503  ")
        .await
        .unwrap();
    let watch = google.load_search_watch(SHOP).await.unwrap().unwrap();
    assert_eq!(watch.attempts, 1);
    assert_eq!(
        watch.last_error.as_deref(),
        Some("Google is unavailable: 503")
    );
    assert_eq!(watch.checked_at, checked_at);
    let first_retry = watch.next_at;
    google
        .record_search_failure(GOOGLE_ACTOR, SHOP, SITE, "again")
        .await
        .unwrap();
    let watch = google.load_search_watch(SHOP).await.unwrap().unwrap();
    assert_eq!(watch.attempts, 2);
    assert!(watch.next_at >= first_retry + 3_600);

    // A pass that works clears them.
    google
        .record_search_check(GOOGLE_ACTOR, SHOP, SITE, false)
        .await
        .unwrap();
    let watch = google.load_search_watch(SHOP).await.unwrap().unwrap();
    assert_eq!(watch.attempts, 0);
    assert_eq!(watch.last_error, None);

    // A failure on another site counts from one, with nothing read yet.
    google
        .record_search_failure(GOOGLE_ACTOR, SHOP, OTHER_SITE, "refused")
        .await
        .unwrap();
    let watch = google.load_search_watch(SHOP).await.unwrap().unwrap();
    assert_eq!(watch.site, OTHER_SITE);
    assert_eq!(watch.attempts, 1);
    assert_eq!(watch.checked_at, 0);
    assert_eq!(watch.aggregate_version, 6);
}

#[tokio::test]
async fn the_shops_due_are_the_connected_ones_with_a_site_and_no_pass_ahead() {
    let (executor, db) = executor_with_pool().await;
    let google = timada_google::Command(executor.clone());
    // Connected, no site yet: nothing to look at.
    google
        .connect(ALICE, SHOP, grant("alice@example.com", b"sealed-1"))
        .await
        .unwrap();
    // No Search Console at all.
    google
        .connect(
            ALICE,
            "tenant-ads",
            Connect {
                refresh_token_sealed: b"sealed-2".to_vec(),
                account: "alice@example.com".to_owned(),
                accesses: vec![ACCESS_ADS.to_owned()],
            },
        )
        .await
        .unwrap();
    catch_up(&executor, &db).await;
    assert!(due(&db, FAR).await.is_empty());

    // A site chosen: due at once, never looked at.
    google.choose_site(ALICE, SHOP, SITE).await.unwrap();
    catch_up(&executor, &db).await;
    catch_up(&executor, &db).await;
    assert_eq!(due(&db, 0).await, vec![shop(SHOP, SITE)]);

    // A pass ended: due again a day later.
    google
        .record_search_check(GOOGLE_ACTOR, SHOP, SITE, false)
        .await
        .unwrap();
    catch_up(&executor, &db).await;
    let watch = google.load_search_watch(SHOP).await.unwrap().unwrap();
    assert!(due(&db, watch.next_at as i64 - 1).await.is_empty());
    assert_eq!(due(&db, watch.next_at as i64).await, vec![shop(SHOP, SITE)]);

    // Another site: due at once, whatever the watch says of the earlier.
    google.choose_site(ALICE, SHOP, OTHER_SITE).await.unwrap();
    catch_up(&executor, &db).await;
    assert_eq!(due(&db, 0).await, vec![shop(SHOP, OTHER_SITE)]);

    // A grant Google no longer honours is not asked with.
    let sealed = google
        .load_connection(SHOP)
        .await
        .unwrap()
        .unwrap()
        .refresh_token_sealed;
    assert!(
        google
            .note_lapsed(GOOGLE_ACTOR, SHOP, &sealed)
            .await
            .unwrap()
    );
    catch_up(&executor, &db).await;
    assert!(due(&db, FAR).await.is_empty());

    // Connected again, the same account: the site is kept, and due.
    google
        .connect(ALICE, SHOP, grant("alice@example.com", b"sealed-3"))
        .await
        .unwrap();
    catch_up(&executor, &db).await;
    assert_eq!(due(&db, FAR).await, vec![shop(SHOP, OTHER_SITE)]);
    // Read a page at a time, by shop.
    assert!(searches_due(&db, FAR, SHOP, 100).await.unwrap().is_empty());

    google.disconnect(ALICE, SHOP).await.unwrap();
    catch_up(&executor, &db).await;
    assert!(due(&db, FAR).await.is_empty());
}

#[tokio::test]
async fn a_period_adds_up_its_days_queries_and_pages() {
    let (executor, db) = executor_with_pool().await;
    let google = timada_google::Command(executor.clone());
    google
        .record_search_day(GOOGLE_ACTOR, SHOP, SITE, day("2026-11-19"))
        .await
        .unwrap();
    let mut second = day("2026-11-20");
    second.numbers.clicks = 30;
    second.numbers.impressions = 600;
    second.numbers.position_milli = 4_000;
    second.queries = vec![
        row("arbre à chat", 20, 300, 2_000),
        row("arbre à chat xxl", 20, 400, 3_000),
    ];
    second.pages = vec![
        row("https://shop.example/arbres-a-chat", 25, 500, 3_500),
        row("https://shop.example/", 5, 100, 9_000),
    ];
    google
        .record_search_day(GOOGLE_ACTOR, SHOP, SITE, second)
        .await
        .unwrap();
    // Another site's and another shop's days stay out of it.
    google
        .record_search_day(GOOGLE_ACTOR, SHOP, OTHER_SITE, day("2026-11-20"))
        .await
        .unwrap();
    google
        .record_search_day(GOOGLE_ACTOR, "tenant-other", SITE, day("2026-11-20"))
        .await
        .unwrap();
    catch_up(&executor, &db).await;
    // Delivery may repeat: the rows do not drift.
    catch_up(&executor, &db).await;

    assert_eq!(
        search_days_held(&db, SHOP, SITE, "2026-11-01")
            .await
            .unwrap(),
        vec!["2026-11-19", "2026-11-20"]
    );
    assert_eq!(
        search_days_held(&db, SHOP, SITE, "2026-11-20")
            .await
            .unwrap(),
        vec!["2026-11-20"]
    );
    assert_eq!(
        last_search_day(&db, SHOP, SITE).await.unwrap().as_deref(),
        Some("2026-11-20")
    );
    assert_eq!(
        last_search_day(&db, SHOP, "sc-domain:none.example")
            .await
            .unwrap(),
        None
    );

    let totals = search_totals(&db, SHOP, SITE, "2026-11-01", "2026-11-30")
        .await
        .unwrap();
    assert_eq!(totals.days, 2);
    assert_eq!(totals.clicks, 40);
    assert_eq!(totals.impressions, 800);
    // 40 clicks over 800 impressions; position 8 over 200 and 4 over 600.
    assert_eq!(totals.figures().click_rate_bps(), Some(500));
    assert_eq!(totals.figures().position_tenths(), Some(50));
    let one = search_totals(&db, SHOP, SITE, "2026-11-19", "2026-11-19")
        .await
        .unwrap();
    assert_eq!((one.days, one.clicks), (1, 10));
    let none = search_totals(&db, SHOP, SITE, "2026-10-01", "2026-10-31")
        .await
        .unwrap();
    assert_eq!(none.days, 0);
    assert_eq!(none.figures().click_rate_bps(), None);
    assert_eq!(none.figures().position_tenths(), None);

    // The most clicked first, then the most shown.
    let queries = top_queries(&db, SHOP, SITE, "2026-11-01", "2026-11-30", 10)
        .await
        .unwrap();
    let read: Vec<_> = queries
        .iter()
        .map(|line| (line.key.as_str(), line.clicks, line.impressions))
        .collect();
    assert_eq!(
        read,
        vec![
            ("arbre à chat", 26, 400),
            ("arbre à chat xxl", 20, 400),
            ("griffoir", 2, 50),
        ]
    );
    // Position 4 over 100 impressions and 2 over 300: 2.5.
    assert_eq!(queries[0].figures().position_tenths(), Some(25));
    assert_eq!(queries[0].figures().click_rate_bps(), Some(650));
    assert_eq!(
        top_queries(&db, SHOP, SITE, "2026-11-01", "2026-11-30", 1)
            .await
            .unwrap()
            .len(),
        1
    );
    let pages = top_pages(&db, SHOP, SITE, "2026-11-20", "2026-11-20", 10)
        .await
        .unwrap();
    let read: Vec<_> = pages
        .iter()
        .map(|line| (line.key.as_str(), line.clicks))
        .collect();
    assert_eq!(
        read,
        vec![
            ("https://shop.example/arbres-a-chat", 25),
            ("https://shop.example/", 5),
        ]
    );
}

#[tokio::test]
async fn the_history_kept_is_bounded() {
    let (executor, db) = executor_with_pool().await;
    let google = timada_google::Command(executor.clone());
    let latest = "2026-11-20";
    let old_rows = day_before(latest, KEPT_ROW_DAYS + 1);
    let kept_rows = day_before(latest, KEPT_ROW_DAYS);
    let old_day = day_before(latest, KEPT_DAYS + 1);
    for held in [&old_day, &old_rows, &kept_rows] {
        google
            .record_search_day(GOOGLE_ACTOR, SHOP, SITE, day(held))
            .await
            .unwrap();
    }
    catch_up(&executor, &db).await;
    assert_eq!(
        search_days_held(&db, SHOP, SITE, "2000-01-01")
            .await
            .unwrap()
            .len(),
        3
    );

    // A new day drops what is older than the history kept: a site's
    // numbers for sixteen months, its queries and pages for three.
    let mut long = day(latest);
    long.queries = (0..MAX_SEARCH_ROWS + 20)
        .map(|n| row(&format!("query {n:04}"), 1, 10, 5_000))
        .chain([row("", 1, 1, 1_000), row(&"x".repeat(2_000), 1, 1, 1_000)])
        .collect();
    google
        .record_search_day(GOOGLE_ACTOR, SHOP, SITE, long)
        .await
        .unwrap();
    catch_up(&executor, &db).await;
    assert_eq!(
        search_days_held(&db, SHOP, SITE, "2000-01-01")
            .await
            .unwrap(),
        vec![old_rows.clone(), kept_rows.clone(), latest.to_owned()]
    );
    let rows_on = |day: String| {
        let db = db.clone();
        async move {
            top_queries(&db, SHOP, SITE, &day, &day, 1_000)
                .await
                .unwrap()
                .len()
        }
    };
    assert_eq!(rows_on(old_rows).await, 0);
    assert_eq!(rows_on(kept_rows).await, 2);
    // A day keeps its most clicked rows only.
    assert_eq!(rows_on(latest.to_owned()).await, MAX_SEARCH_ROWS);
}
