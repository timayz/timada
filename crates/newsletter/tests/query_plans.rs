//! Probes of the sending's reads under Turso's planner (issues #336, #469):
//! each seeds a few thousand rows in a database file opened for several
//! writers, prints `EXPLAIN QUERY PLAN` and a timing for the query as the
//! sending runs it and for the alternatives, and checks that the sending's
//! version answers the same. Ignored: they take seconds and their report is
//! their point. Run them with
//! `cargo test -p timada-newsletter --test query_plans -- --ignored --nocapture`.

use std::time::Instant;

use timada_core::db::{Connection, Db, IntoParams, Turso, execute, fetch_all};
use timada_core::{Share, shard_of};
use timada_newsletter::{
    SubscriberRow, edition_progress, undelivered_in_zone, undelivered_subscribers,
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
/// another edition to keep the indexes honest. The counts are not kept:
/// nothing here reads them but [`edition_progress_plan`], which seeds its
/// own.
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
                requested_at, confirmed_at, unsubscribed_at, updated_at, time_zone, shard)
             VALUES (?1, ?2, ?3, ?4, 'form', ?5, ?6, ?6, NULL, ?6, ?7, ?8)",
            (
                TENANT,
                id.as_str(),
                format!("s{n}@example.com"),
                language,
                status,
                n as i64,
                ZONES[n % ZONES.len()],
                i64::from(shard_of(&id)),
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
                requested_at, confirmed_at, unsubscribed_at, updated_at, time_zone, shard)
             VALUES ('other', ?1, ?2, 'en', 'form', 'confirmed', ?3, ?3, NULL, ?3, NULL, ?4)",
            (
                id.as_str(),
                format!("o{n}@example.com"),
                n as i64,
                i64::from(shard_of(&id)),
            ),
        )
        .await
        .unwrap();
    }
    execute(conn, "COMMIT", ()).await.unwrap();
}

/// What one node's walk read before #469: everyone owed, from the top.
const FROM_THE_TOP: &str = "SELECT s.* FROM newsletter_subscribers s
         WHERE s.tenant_id = ?1 AND s.status = 'confirmed'
           AND (?3 IS NULL OR s.language = ?3)
           AND NOT EXISTS (
               SELECT 1 FROM newsletter_deliveries d INDEXED BY sqlite_autoindex_newsletter_deliveries_1
               WHERE d.tenant_id = s.tenant_id AND d.edition_id = ?2
                 AND d.subscriber_id = s.id)
         ORDER BY s.id
         LIMIT ?4";

/// The walk without naming the subscribers' index.
const WALK_UNINDEXED: &str = "SELECT s.* FROM newsletter_subscribers s
         WHERE s.tenant_id = ?1 AND s.id > ?4 AND s.status = 'confirmed'
           AND (?3 IS NULL OR s.language = ?3)
           AND s.shard % ?5 = ?6
           AND NOT EXISTS (
               SELECT 1 FROM newsletter_deliveries d INDEXED BY sqlite_autoindex_newsletter_deliveries_1
               WHERE d.tenant_id = s.tenant_id AND d.edition_id = ?2
                 AND d.subscriber_id = s.id)
         ORDER BY s.id
         LIMIT ?7";

/// The walk as the sending runs it.
const WALK: &str = "SELECT s.* FROM newsletter_subscribers s
            INDEXED BY sqlite_autoindex_newsletter_subscribers_1
         WHERE s.tenant_id = ?1 AND s.id > ?4 AND s.status = 'confirmed'
           AND (?3 IS NULL OR s.language = ?3)
           AND s.shard % ?5 = ?6
           AND NOT EXISTS (
               SELECT 1 FROM newsletter_deliveries d INDEXED BY sqlite_autoindex_newsletter_deliveries_1
               WHERE d.tenant_id = s.tenant_id AND d.edition_id = ?2
                 AND d.subscriber_id = s.id)
         ORDER BY s.id
         LIMIT ?7";

#[tokio::test]
#[ignore = "a probe: seeds thousands of rows and prints plans and timings"]
async fn undelivered_subscribers_plan() {
    let (store, db) = database().await;
    let conn = store.connect().unwrap();
    seed(&conn).await;
    let share = Share { index: 1, of: 3 };
    // Where a walk stands halfway through the mailed half.
    let after = format!("subscriber-{:06}", SUBSCRIBERS / 4);

    println!(
        "undelivered_subscribers: {SUBSCRIBERS} subscribers, {} mailed",
        SUBSCRIBERS / 2
    );
    report_plan(
        &conn,
        "from the top, every node's",
        FROM_THE_TOP,
        (TENANT, EDITION, Some("en"), 50i64),
    )
    .await;
    timed("from the top", 10, || {
        fetch_all::<SubscriberRow>(&conn, FROM_THE_TOP, (TENANT, EDITION, Some("en"), 50i64))
    })
    .await
    .unwrap();

    let params = || {
        (
            TENANT,
            EDITION,
            Some("en"),
            after.as_str(),
            share.of as i64,
            share.index as i64,
            50i64,
        )
    };
    report_plan(&conn, "the walk, unindexed", WALK_UNINDEXED, params()).await;
    let unindexed: Vec<SubscriberRow> = timed("the walk, unindexed", 10, || {
        fetch_all(&conn, WALK_UNINDEXED, params())
    })
    .await
    .unwrap();
    let plan = report_plan(&conn, "the walk", WALK, params()).await;
    assert_step(
        &plan,
        "INDEX sqlite_autoindex_newsletter_subscribers_1 (tenant_id=? AND id>?)",
    );
    assert_step(&plan, DELIVERY_BY_KEY);
    let walked: Vec<SubscriberRow> = timed("the walk", 10, || fetch_all(&conn, WALK, params()))
        .await
        .unwrap();
    assert_eq!(walked, unindexed);
    assert_eq!(walked.len(), 50);
    assert!(walked.iter().all(|s| share.owns(&s.id) && s.id > after));

    let page = timed("the sending's query", 10, || {
        undelivered_subscribers(&db, TENANT, EDITION, Some("en"), share, &after, 50)
    })
    .await
    .unwrap();
    assert_eq!(page, walked);
    let in_zone = timed("the sending's query, in a zone", 10, || {
        undelivered_in_zone(
            &db,
            TENANT,
            EDITION,
            Some("en"),
            "Europe/London",
            "Europe/Paris",
            share,
            &after,
            50,
        )
    })
    .await
    .unwrap();
    assert_eq!(in_zone.len(), 50);
    assert!(
        in_zone
            .iter()
            .all(|s| s.time_zone.as_deref() == Some("Europe/Paris") && share.owns(&s.id))
    );
}

#[tokio::test]
#[ignore = "a probe: seeds thousands of rows and prints plans and timings"]
async fn edition_progress_plan() {
    let (store, db) = database().await;
    let conn = store.connect().unwrap();
    execute(&conn, "BEGIN", ()).await.unwrap();
    for (n, zone) in ZONES.iter().enumerate() {
        let zone = zone.unwrap_or_default();
        for language in ["en", "fr"] {
            execute(
                &conn,
                "INSERT INTO newsletter_audience (tenant_id, language, time_zone, confirmed)
                 VALUES (?1, ?2, ?3, ?4)",
                (TENANT, language, zone, (SUBSCRIBERS / 8) as i64),
            )
            .await
            .unwrap();
        }
        execute(
            &conn,
            "INSERT INTO newsletter_edition_zones (tenant_id, edition_id, time_zone, mailed)
             VALUES (?1, ?2, ?3, ?4)",
            (TENANT, EDITION, zone, (n * 100) as i64),
        )
        .await
        .unwrap();
    }
    execute(&conn, "COMMIT", ()).await.unwrap();

    let plan = report_plan(
        &conn,
        "an edition's mails by zone",
        "SELECT time_zone, mailed FROM newsletter_edition_zones
         WHERE tenant_id = ?1 AND edition_id = ?2",
        (TENANT, EDITION),
    )
    .await;
    assert_step(
        &plan,
        "INDEX sqlite_autoindex_newsletter_edition_zones_1 (tenant_id=? AND edition_id=?)",
    );
    let plan = report_plan(
        &conn,
        "the audience by zone",
        "SELECT time_zone, confirmed FROM newsletter_audience
         WHERE tenant_id = ?1 AND (?2 IS NULL OR language = ?2)",
        (TENANT, Some("en")),
    )
    .await;
    assert_step(
        &plan,
        "INDEX sqlite_autoindex_newsletter_audience_1 (tenant_id=?",
    );
    let progress = timed("the sending's query", 10, || {
        edition_progress(&db, TENANT, EDITION, Some("en"), "Europe/London")
    })
    .await
    .unwrap();
    assert_eq!(progress.len(), ZONES.len());
    assert!(
        progress
            .iter()
            .all(|zone| zone.audience == (SUBSCRIBERS / 8) as i64)
    );
}
