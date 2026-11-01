//! Probes of the per-request joins under Turso's planner (issue #336): each
//! seeds a few thousand rows in a database file opened for several writers,
//! prints `EXPLAIN QUERY PLAN` and a timing for the query as the page runs
//! it and for the alternatives, and checks that the page's version answers
//! the same. Ignored: they take seconds and their report is their point.
//! Run them with
//! `cargo test -p timada-newsletter --test query_plans -- --ignored --nocapture`.

use std::time::Instant;

use timada_core::db::{Connection, Db, IntoParams, Turso, execute, fetch_all};
use timada_newsletter::{
    SubscriberRow, ZoneProgress, campaign_row, edition_progress, editions_of_campaign,
    undelivered_in_zone, undelivered_subscribers,
};

/// A database file opened for several writers, with the newsletter schema.
async fn database() -> (Turso, Db) {
    let store = Turso::temporary_concurrent().await.unwrap();
    let mut conn = store.connect().unwrap();
    timada_newsletter::MIGRATIONS
        .to_latest(&mut conn)
        .await
        .map_err(|(_, err)| err)
        .unwrap();
    (store.clone(), Db::from(store))
}

/// The `detail` column of `EXPLAIN QUERY PLAN sql`, one line per step.
async fn plan(conn: &Connection, sql: &str, params: impl IntoParams) -> Vec<String> {
    let rows: Vec<(i64, i64, i64, String)> =
        fetch_all(conn, &format!("EXPLAIN QUERY PLAN {sql}"), params)
            .await
            .unwrap();
    rows.into_iter().map(|(_, _, _, detail)| detail).collect()
}

/// Prints the plan of `sql` under `title`, and returns it.
async fn report_plan(
    conn: &Connection,
    title: &str,
    sql: &str,
    params: impl IntoParams,
) -> Vec<String> {
    println!("--- {title}");
    let plan = plan(conn, sql, params).await;
    for line in &plan {
        println!("    {line}");
    }
    plan
}

/// Fails unless one step of `plan` holds `step`: the deliveries looked up
/// by their whole key, say (whether or not the index covers the columns).
fn assert_step(plan: &[String], step: &str) {
    assert!(
        plan.iter().any(|line| line.contains(step)),
        "no step {step:?} in the plan:\n{}",
        plan.join("\n")
    );
}

/// The lookup of one subscriber's delivery, in the plans.
const DELIVERY_BY_KEY: &str = "INDEX sqlite_autoindex_newsletter_deliveries_1 \
                               (tenant_id=? AND edition_id=? AND subscriber_id=?)";

/// Runs `run` `rounds` times and prints the mean wall time under `title`.
async fn timed<F, Fut, T>(title: &str, rounds: u32, mut run: F) -> T
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = T>,
{
    let mut last = None;
    let started = Instant::now();
    for _ in 0..rounds {
        last = Some(run().await);
    }
    let each = started.elapsed() / rounds;
    println!("    {title}: {each:?} per run ({rounds} runs)");
    last.unwrap()
}

const TENANT: &str = "acme";
const EDITION: &str = "edition-1";
const SUBSCRIBERS: usize = 5_000;
const ZONES: [Option<&str>; 4] = [
    None,
    Some("Europe/Paris"),
    Some("America/New_York"),
    Some("Asia/Tokyo"),
];

/// Seeds `SUBSCRIBERS` subscribers, nine in ten confirmed and one in ten
/// unsubscribed, half of them mailed by the edition (one in twenty of
/// those skipped), plus another tenant's subscribers and deliveries of
/// another edition to keep the indexes honest.
async fn seed(conn: &Connection) {
    execute(conn, "BEGIN", ()).await.unwrap();
    execute(
        conn,
        "INSERT INTO newsletter_campaigns (tenant_id, id, subject, language, status,
            created_at, sent_at, finished_at, updated_at, cadence)
         VALUES (?1, 'campaign-1', 'News', 'en', 'sending', 1, 1, NULL, 1, 'monthly')",
        (TENANT,),
    )
    .await
    .unwrap();
    execute(
        conn,
        "INSERT INTO newsletter_editions (tenant_id, id, campaign_id, cycle, status, opened_at,
            closed_at)
         VALUES (?1, ?2, 'campaign-1', '2026-11-01', 'open', 1, NULL)",
        (TENANT, EDITION),
    )
    .await
    .unwrap();
    for n in 0..SUBSCRIBERS {
        let id = format!("subscriber-{n:06}");
        let status = if n % 10 == 9 {
            "unsubscribed"
        } else {
            "confirmed"
        };
        let language = if n % 3 == 0 { Some("fr") } else { Some("en") };
        execute(
            conn,
            "INSERT INTO newsletter_subscribers (tenant_id, id, email, language, source, status,
                requested_at, confirmed_at, unsubscribed_at, updated_at, time_zone)
             VALUES (?1, ?2, ?3, ?4, 'form', ?5, ?6, ?6, NULL, ?6, ?7)",
            (
                TENANT,
                id.as_str(),
                format!("s{n}@example.com"),
                language,
                status,
                n as i64,
                ZONES[n % ZONES.len()],
            ),
        )
        .await
        .unwrap();
        if n % 2 == 0 {
            execute(
                conn,
                "INSERT INTO newsletter_deliveries
                    (tenant_id, campaign_id, edition_id, subscriber_id, failed, skipped)
                 VALUES (?1, 'campaign-1', ?2, ?3, 0, ?4)",
                (TENANT, EDITION, id.as_str(), i64::from(n % 20 == 0)),
            )
            .await
            .unwrap();
        }
        if n % 4 == 0 {
            execute(
                conn,
                "INSERT INTO newsletter_deliveries
                    (tenant_id, campaign_id, edition_id, subscriber_id, failed, skipped)
                 VALUES (?1, 'campaign-0', 'edition-0', ?2, 0, 0)",
                (TENANT, id.as_str()),
            )
            .await
            .unwrap();
        }
        execute(
            conn,
            "INSERT INTO newsletter_subscribers (tenant_id, id, email, language, source, status,
                requested_at, confirmed_at, unsubscribed_at, updated_at, time_zone)
             VALUES ('other', ?1, ?2, 'en', 'form', 'confirmed', ?3, ?3, NULL, ?3, NULL)",
            (id.as_str(), format!("o{n}@example.com"), n as i64),
        )
        .await
        .unwrap();
    }
    execute(conn, "COMMIT", ()).await.unwrap();
}

const LEFT_JOIN: &str = "SELECT zone, SUM(mailed) AS delivered, SUM(waiting) AS owed FROM (
            SELECT COALESCE(s.time_zone, ?4) AS zone,
                CASE WHEN d.subscriber_id IS NOT NULL AND d.skipped = 0 THEN 1 ELSE 0 END
                    AS mailed,
                CASE WHEN d.subscriber_id IS NULL AND s.status = 'confirmed'
                     AND (?3 IS NULL OR s.language = ?3) THEN 1 ELSE 0 END AS waiting
            FROM newsletter_subscribers s
            LEFT JOIN newsletter_deliveries d
              ON d.tenant_id = s.tenant_id AND d.edition_id = ?2 AND d.subscriber_id = s.id
            WHERE s.tenant_id = ?1)
         GROUP BY zone
         HAVING SUM(mailed) + SUM(waiting) > 0
         ORDER BY zone";

/// The join as the page ran it, the deliveries' primary key forced.
const LEFT_JOIN_INDEXED: &str = "SELECT zone, SUM(mailed) AS delivered, SUM(waiting) AS owed FROM (
            SELECT COALESCE(s.time_zone, ?4) AS zone,
                CASE WHEN d.subscriber_id IS NOT NULL AND d.skipped = 0 THEN 1 ELSE 0 END
                    AS mailed,
                CASE WHEN d.subscriber_id IS NULL AND s.status = 'confirmed'
                     AND (?3 IS NULL OR s.language = ?3) THEN 1 ELSE 0 END AS waiting
            FROM newsletter_subscribers s
            LEFT JOIN newsletter_deliveries d INDEXED BY sqlite_autoindex_newsletter_deliveries_1
              ON d.tenant_id = s.tenant_id AND d.edition_id = ?2 AND d.subscriber_id = s.id
            WHERE s.tenant_id = ?1)
         GROUP BY zone
         HAVING SUM(mailed) + SUM(waiting) > 0
         ORDER BY zone";

#[tokio::test]
#[ignore = "a probe: seeds thousands of rows and prints plans and timings"]
async fn edition_progress_plan() {
    let (store, db) = database().await;
    let conn = store.connect().unwrap();
    seed(&conn).await;
    let params = || (TENANT, EDITION, Some("en"), "Europe/London");

    println!(
        "edition_progress: {SUBSCRIBERS} subscribers, {} mailed",
        SUBSCRIBERS / 2
    );
    report_plan(
        &conn,
        "LEFT JOIN subscribers -> deliveries",
        LEFT_JOIN,
        params(),
    )
    .await;
    // Reported only: this is the plan the planner picks today (the wrong
    // index), and it changes once tursodatabase/turso#8664 is fixed.
    let joined: Vec<ZoneProgress> = timed("LEFT JOIN", 1, || fetch_all(&conn, LEFT_JOIN, params()))
        .await
        .unwrap();
    assert_eq!(joined.len(), ZONES.len());

    let plan = report_plan(
        &conn,
        "LEFT JOIN, INDEXED BY the primary key",
        LEFT_JOIN_INDEXED,
        params(),
    )
    .await;
    assert_step(&plan, &format!("{DELIVERY_BY_KEY} LEFT-JOIN"));
    let indexed: Vec<ZoneProgress> = timed("LEFT JOIN INDEXED BY", 10, || {
        fetch_all(&conn, LEFT_JOIN_INDEXED, params())
    })
    .await
    .unwrap();
    assert_eq!(indexed, joined);

    report_plan(
        &conn,
        "subscribers alone",
        "SELECT id, time_zone, status, language FROM newsletter_subscribers WHERE tenant_id = ?1",
        (TENANT,),
    )
    .await;
    report_plan(
        &conn,
        "deliveries alone",
        "SELECT subscriber_id, skipped FROM newsletter_deliveries
         WHERE tenant_id = ?1 AND edition_id = ?2",
        (TENANT, EDITION),
    )
    .await;
    let two_scans = timed("two scans joined here", 10, || async {
        let subscribers: Vec<(String, Option<String>, String, Option<String>)> = fetch_all(
            &conn,
            "SELECT id, time_zone, status, language FROM newsletter_subscribers
             WHERE tenant_id = ?1",
            (TENANT,),
        )
        .await
        .unwrap();
        let deliveries: std::collections::HashMap<String, bool> = fetch_all::<(String, bool)>(
            &conn,
            "SELECT subscriber_id, skipped FROM newsletter_deliveries
             WHERE tenant_id = ?1 AND edition_id = ?2",
            (TENANT, EDITION),
        )
        .await
        .unwrap()
        .into_iter()
        .collect();
        subscribers.len() + deliveries.len()
    })
    .await;
    assert_eq!(two_scans, SUBSCRIBERS + SUBSCRIBERS / 2);

    let page = timed("the page's query", 10, || {
        edition_progress(&db, TENANT, EDITION, Some("en"), "Europe/London")
    })
    .await
    .unwrap();
    assert_eq!(page, joined);
}

const NOT_EXISTS: &str = "SELECT s.* FROM newsletter_subscribers s
         WHERE s.tenant_id = ?1 AND s.status = 'confirmed'
           AND (?3 IS NULL OR s.language = ?3)
           AND NOT EXISTS (
               SELECT 1 FROM newsletter_deliveries d
               WHERE d.tenant_id = s.tenant_id AND d.edition_id = ?2
                 AND d.subscriber_id = s.id)
         ORDER BY s.id
         LIMIT ?4";

const NOT_EXISTS_INDEXED: &str = "SELECT s.* FROM newsletter_subscribers s
         WHERE s.tenant_id = ?1 AND s.status = 'confirmed'
           AND (?3 IS NULL OR s.language = ?3)
           AND NOT EXISTS (
               SELECT 1 FROM newsletter_deliveries d INDEXED BY sqlite_autoindex_newsletter_deliveries_1
               WHERE d.tenant_id = s.tenant_id AND d.edition_id = ?2
                 AND d.subscriber_id = s.id)
         ORDER BY s.id
         LIMIT ?4";

#[tokio::test]
#[ignore = "a probe: seeds thousands of rows and prints plans and timings"]
async fn undelivered_subscribers_plan() {
    let (store, db) = database().await;
    let conn = store.connect().unwrap();
    seed(&conn).await;
    let params = || (TENANT, EDITION, Some("en"), 50i64);

    println!(
        "undelivered_subscribers: {SUBSCRIBERS} subscribers, {} mailed",
        SUBSCRIBERS / 2
    );
    report_plan(&conn, "NOT EXISTS a delivery", NOT_EXISTS, params()).await;
    let plain: Vec<SubscriberRow> =
        timed("NOT EXISTS", 1, || fetch_all(&conn, NOT_EXISTS, params()))
            .await
            .unwrap();
    assert_eq!(plain.len(), 50);

    let plan = report_plan(
        &conn,
        "NOT EXISTS, INDEXED BY the primary key",
        NOT_EXISTS_INDEXED,
        params(),
    )
    .await;
    assert_step(&plan, DELIVERY_BY_KEY);
    let indexed: Vec<SubscriberRow> = timed("NOT EXISTS INDEXED BY", 10, || {
        fetch_all(&conn, NOT_EXISTS_INDEXED, params())
    })
    .await
    .unwrap();
    assert_eq!(indexed, plain);

    let page = timed("the sweep's query", 10, || {
        undelivered_subscribers(&db, TENANT, EDITION, Some("en"), 50)
    })
    .await
    .unwrap();
    assert_eq!(page, plain);
    let in_zone = timed("the sweep's query, in a zone", 10, || {
        undelivered_in_zone(
            &db,
            TENANT,
            EDITION,
            Some("en"),
            "Europe/London",
            "Europe/Paris",
            50,
        )
    })
    .await
    .unwrap();
    assert_eq!(in_zone.len(), 50);
    assert!(
        in_zone
            .iter()
            .all(|s| s.time_zone.as_deref() == Some("Europe/Paris"))
    );
}

#[tokio::test]
#[ignore = "a probe: seeds thousands of rows and prints plans and timings"]
async fn delivery_counts_plan() {
    let (store, db) = database().await;
    let conn = store.connect().unwrap();
    seed(&conn).await;

    println!(
        "delivery counts: {} deliveries of the campaign",
        SUBSCRIBERS / 2
    );
    report_plan(
        &conn,
        "deliveries of an edition",
        "SELECT subscriber_id, skipped FROM newsletter_deliveries
         WHERE tenant_id = ?1 AND edition_id = ?2",
        (TENANT, EDITION),
    )
    .await;
    let plan = report_plan(
        &conn,
        "deliveries of an edition, INDEXED BY the primary key",
        "SELECT subscriber_id, skipped FROM newsletter_deliveries
            INDEXED BY sqlite_autoindex_newsletter_deliveries_1
         WHERE tenant_id = ?1 AND edition_id = ?2",
        (TENANT, EDITION),
    )
    .await;
    assert_step(
        &plan,
        "SEARCH newsletter_deliveries USING INDEX sqlite_autoindex_newsletter_deliveries_1 \
         (tenant_id=? AND edition_id=?)",
    );
    report_plan(
        &conn,
        "a campaign's counts",
        "SELECT (SELECT COUNT(*) FROM newsletter_deliveries d
                 WHERE d.tenant_id = c.tenant_id AND d.campaign_id = c.id AND d.failed = 1)
         FROM newsletter_campaigns c WHERE c.tenant_id = ?1 AND c.id = ?2",
        (TENANT, "campaign-1"),
    )
    .await;
    let plan = report_plan(
        &conn,
        "a campaign's counts, INDEXED BY the campaign index",
        "SELECT (SELECT COUNT(*) FROM newsletter_deliveries d INDEXED BY newsletter_deliveries_campaign
                 WHERE d.tenant_id = c.tenant_id AND d.campaign_id = c.id AND d.failed = 1)
         FROM newsletter_campaigns c WHERE c.tenant_id = ?1 AND c.id = ?2",
        (TENANT, "campaign-1"),
    )
    .await;
    assert_step(
        &plan,
        "SEARCH d USING INDEX newsletter_deliveries_campaign (tenant_id=? AND campaign_id=?)",
    );
    let campaign = timed("campaign_row", 10, || {
        campaign_row(&db, TENANT, "campaign-1")
    })
    .await
    .unwrap()
    .unwrap();
    assert_eq!(
        campaign.delivered + campaign.skipped,
        (SUBSCRIBERS / 2) as i64
    );
    report_plan(
        &conn,
        "an edition's counts",
        "SELECT (SELECT COUNT(*) FROM newsletter_deliveries d
                 WHERE d.tenant_id = e.tenant_id AND d.edition_id = e.id AND d.failed = 1)
         FROM newsletter_editions e WHERE e.tenant_id = ?1 AND e.campaign_id = ?2",
        (TENANT, "campaign-1"),
    )
    .await;
    let plan = report_plan(
        &conn,
        "an edition's counts, INDEXED BY the primary key",
        "SELECT (SELECT COUNT(*) FROM newsletter_deliveries d
                    INDEXED BY sqlite_autoindex_newsletter_deliveries_1
                 WHERE d.tenant_id = e.tenant_id AND d.edition_id = e.id AND d.failed = 1)
         FROM newsletter_editions e WHERE e.tenant_id = ?1 AND e.campaign_id = ?2",
        (TENANT, "campaign-1"),
    )
    .await;
    assert_step(
        &plan,
        "SEARCH d USING INDEX sqlite_autoindex_newsletter_deliveries_1 \
         (tenant_id=? AND edition_id=?)",
    );
    let editions = timed("editions_of_campaign", 10, || {
        editions_of_campaign(&db, TENANT, "campaign-1", 10)
    })
    .await
    .unwrap();
    assert_eq!(
        editions[0].delivered + editions[0].skipped,
        (SUBSCRIBERS / 2) as i64
    );
}
