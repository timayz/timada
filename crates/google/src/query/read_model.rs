//! The subscription that keeps the six tables current.
//!
//! A connection's and a watch's handler reload the strict write model and
//! rewrite the shop's row, so delivery may repeat (the subscription is
//! at-least-once) without the row drifting. A day's rows come whole in the
//! event that records it: they replace the day's, in one transaction, and
//! the same transaction drops what is older than the history kept, so the
//! tables stay bounded without a sweep of their own.

use evento::{
    Executor,
    metadata::{Event, RawEvent},
    subscription::{Context, SubscriptionBuilder},
};
use timada_core::db::{Db, Value, execute, params_from_iter};

use crate::aggregator::{Connection, DayFetched, PageQueryRow, SearchRow, SearchWatch};
use crate::command::{connection_projection, search_watch_projection};
use crate::value_object::{ACCESS_SEARCH_CONSOLE, KEPT_DAYS, KEPT_ROW_DAYS, day_before};

/// The cursor identity of the read-model subscription, in the store.
pub const READ_MODEL_SUBSCRIPTION: &str = "timada-google/read-models";

/// How many rows one `INSERT` writes: a statement costs more than a row,
/// and a day gives up to a thousand.
const ROWS_PER_INSERT: usize = 50;

/// Builds the subscription over every routing key of the platform tenant,
/// where the connections and what is read through them live. Start it with
/// `.start(&store)` and keep the handle for shutdown.
pub fn read_model_subscription<E: Executor>(db: Db) -> SubscriptionBuilder<E> {
    SubscriptionBuilder::new(READ_MODEL_SUBSCRIPTION)
        .data(db)
        .handler(on_connection_event())
        .handler(on_watch_event())
        .handler(on_day_fetched())
        .any_routing_key()
        .tenant("")
}

#[evento::subscription_all]
async fn on_connection_event<E: Executor>(
    context: &Context<'_, E>,
    event: RawEvent<Connection>,
) -> anyhow::Result<()> {
    let db: Db = context.extract();
    let Some(connection) = connection_projection()
        .load(&event.aggregate_id)
        .execute(context.executor)
        .await?
    else {
        return Ok(());
    };
    let site = connection
        .site
        .as_deref()
        .filter(|_| connection.covers(ACCESS_SEARCH_CONSOLE));
    match site {
        Some(site) => {
            db.execute(
                "INSERT INTO google_connections (tenant_id, site, lapsed) VALUES (?1, ?2, ?3)
                 ON CONFLICT (tenant_id) DO UPDATE SET
                    site = excluded.site, lapsed = excluded.lapsed",
                (
                    connection.tenant_id.as_str(),
                    site,
                    i64::from(connection.lapsed),
                ),
            )
            .await?;
        }
        None => {
            db.execute(
                "DELETE FROM google_connections WHERE tenant_id = ?1",
                (connection.tenant_id.as_str(),),
            )
            .await?;
        }
    }
    Ok(())
}

#[evento::subscription_all]
async fn on_watch_event<E: Executor>(
    context: &Context<'_, E>,
    event: RawEvent<SearchWatch>,
) -> anyhow::Result<()> {
    let db: Db = context.extract();
    let Some(watch) = search_watch_projection()
        .load(&event.aggregate_id)
        .execute(context.executor)
        .await?
    else {
        return Ok(());
    };
    db.execute(
        "INSERT INTO google_search_watch (tenant_id, site, next_at) VALUES (?1, ?2, ?3)
         ON CONFLICT (tenant_id) DO UPDATE SET site = excluded.site, next_at = excluded.next_at",
        (
            watch.tenant_id.as_str(),
            watch.site.as_str(),
            watch.next_at as i64,
        ),
    )
    .await?;
    Ok(())
}

#[evento::subscription]
async fn on_day_fetched<E: Executor>(
    context: &Context<'_, E>,
    event: Event<DayFetched>,
) -> anyhow::Result<()> {
    let db: Db = context.extract();
    db.write(&event.data, |tx, day| {
        Box::pin(async move {
            let key = (day.tenant_id.as_str(), day.site.as_str(), day.day.as_str());
            execute(
                tx,
                "INSERT INTO google_search_days
                    (tenant_id, site, day, clicks, impressions, position_milli)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT (tenant_id, site, day) DO UPDATE SET
                    clicks = excluded.clicks, impressions = excluded.impressions,
                    position_milli = excluded.position_milli",
                (
                    key.0,
                    key.1,
                    key.2,
                    day.clicks as i64,
                    day.impressions as i64,
                    i64::from(day.position_milli),
                ),
            )
            .await?;
            for (table, column, rows) in [
                ("google_search_queries", "query", &day.queries),
                ("google_search_pages", "page", &day.pages),
            ] {
                execute(
                    tx,
                    &format!("DELETE FROM {table} WHERE tenant_id = ?1 AND site = ?2 AND day = ?3"),
                    key,
                )
                .await?;
                insert_rows(tx, table, column, key, rows).await?;
                // The history kept, counted back from the day just read.
                execute(
                    tx,
                    &format!("DELETE FROM {table} WHERE tenant_id = ?1 AND site = ?2 AND day < ?3"),
                    (key.0, key.1, day_before(&day.day, KEPT_ROW_DAYS)),
                )
                .await?;
            }
            execute(
                tx,
                "DELETE FROM google_search_page_queries
                 WHERE tenant_id = ?1 AND site = ?2 AND day = ?3",
                key,
            )
            .await?;
            insert_page_queries(tx, key, &day.page_queries).await?;
            execute(
                tx,
                "DELETE FROM google_search_page_queries
                 WHERE tenant_id = ?1 AND site = ?2 AND day < ?3",
                (key.0, key.1, day_before(&day.day, KEPT_ROW_DAYS)),
            )
            .await?;
            execute(
                tx,
                "DELETE FROM google_search_days WHERE tenant_id = ?1 AND site = ?2 AND day < ?3",
                (key.0, key.1, day_before(&day.day, KEPT_DAYS)),
            )
            .await?;
            Ok(())
        })
    })
    .await
}

/// Writes a day's rows, a few statements for the lot. Google may list a
/// key twice under two spellings it then folds: the later one stands.
async fn insert_rows(
    tx: &timada_core::db::Connection,
    table: &str,
    column: &str,
    key: (&str, &str, &str),
    rows: &[SearchRow],
) -> anyhow::Result<()> {
    for chunk in rows.chunks(ROWS_PER_INSERT) {
        let mut values = Vec::with_capacity(chunk.len() * 7);
        let mut tuples = Vec::with_capacity(chunk.len());
        for (at, row) in chunk.iter().enumerate() {
            let n = at * 7;
            tuples.push(format!(
                "(?{}, ?{}, ?{}, ?{}, ?{}, ?{}, ?{})",
                n + 1,
                n + 2,
                n + 3,
                n + 4,
                n + 5,
                n + 6,
                n + 7
            ));
            values.extend([
                Value::from(key.0),
                Value::from(key.1),
                Value::from(key.2),
                Value::from(row.key.as_str()),
                Value::from(row.clicks as i64),
                Value::from(row.impressions as i64),
                Value::from(i64::from(row.position_milli)),
            ]);
        }
        let statement = format!(
            "INSERT INTO {table}
                (tenant_id, site, day, {column}, clicks, impressions, position_milli)
             VALUES {}
             ON CONFLICT (tenant_id, site, day, {column}) DO UPDATE SET
                clicks = excluded.clicks, impressions = excluded.impressions,
                position_milli = excluded.position_milli",
            tuples.join(", ")
        );
        execute(tx, &statement, params_from_iter(values)).await?;
    }
    Ok(())
}

/// Writes the queries a day's pages showed for, a few statements for the
/// lot; a pair listed twice keeps the later figures.
async fn insert_page_queries(
    tx: &timada_core::db::Connection,
    key: (&str, &str, &str),
    rows: &[PageQueryRow],
) -> anyhow::Result<()> {
    for chunk in rows.chunks(ROWS_PER_INSERT) {
        let mut values = Vec::with_capacity(chunk.len() * 8);
        let mut tuples = Vec::with_capacity(chunk.len());
        for (at, row) in chunk.iter().enumerate() {
            let n = at * 8;
            let places: Vec<String> = (1..=8).map(|place| format!("?{}", n + place)).collect();
            tuples.push(format!("({})", places.join(", ")));
            values.extend([
                Value::from(key.0),
                Value::from(key.1),
                Value::from(key.2),
                Value::from(row.page.as_str()),
                Value::from(row.query.as_str()),
                Value::from(row.clicks as i64),
                Value::from(row.impressions as i64),
                Value::from(i64::from(row.position_milli)),
            ]);
        }
        let statement = format!(
            "INSERT INTO google_search_page_queries
                (tenant_id, site, day, page, query, clicks, impressions, position_milli)
             VALUES {}
             ON CONFLICT (tenant_id, site, day, page, query) DO UPDATE SET
                clicks = excluded.clicks, impressions = excluded.impressions,
                position_milli = excluded.position_milli",
            tuples.join(", ")
        );
        execute(tx, &statement, params_from_iter(values)).await?;
    }
    Ok(())
}
