//! The subscription that keeps the tables current.
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

use crate::aggregator::{
    AdsDayFetched, AdsNumbers, AdsWatch, Connection, DayFetched, MerchantRead, MerchantWatch,
    PageQueryRow, SearchRow, SearchWatch,
};
use crate::command::{
    ads_watch_projection, connection_projection, merchant_watch_projection, search_watch_projection,
};
use crate::value_object::{
    ACCESS_ADS, ACCESS_MERCHANT_CENTER, ACCESS_SEARCH_CONSOLE, KEPT_DAYS, KEPT_ROW_DAYS, day_before,
};

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
        .handler(on_ads_watch_event())
        .handler(on_ads_day_fetched())
        .handler(on_merchant_watch_event())
        .handler(on_merchant_read())
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
    let ads = connection
        .ads
        .as_ref()
        .filter(|_| connection.covers(ACCESS_ADS));
    match ads {
        Some(ads) => {
            db.execute(
                "INSERT INTO google_ads_accounts (tenant_id, customer_id, lapsed)
                 VALUES (?1, ?2, ?3)
                 ON CONFLICT (tenant_id) DO UPDATE SET
                    customer_id = excluded.customer_id, lapsed = excluded.lapsed",
                (
                    connection.tenant_id.as_str(),
                    ads.customer_id.as_str(),
                    i64::from(connection.lapsed),
                ),
            )
            .await?;
        }
        None => {
            db.execute(
                "DELETE FROM google_ads_accounts WHERE tenant_id = ?1",
                (connection.tenant_id.as_str(),),
            )
            .await?;
        }
    }
    let merchant = connection
        .merchant
        .as_ref()
        .filter(|_| connection.covers(ACCESS_MERCHANT_CENTER));
    match merchant {
        Some(merchant) => {
            db.execute(
                "INSERT INTO google_merchant_accounts (tenant_id, account_id, lapsed)
                 VALUES (?1, ?2, ?3)
                 ON CONFLICT (tenant_id) DO UPDATE SET
                    account_id = excluded.account_id, lapsed = excluded.lapsed",
                (
                    connection.tenant_id.as_str(),
                    merchant.account_id.as_str(),
                    i64::from(connection.lapsed),
                ),
            )
            .await?;
        }
        None => {
            db.execute(
                "DELETE FROM google_merchant_accounts WHERE tenant_id = ?1",
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

/// How many days of the questions asked are kept: the allowance is a
/// day's.
const KEPT_CALL_DAYS: u32 = 7;

#[evento::subscription_all]
async fn on_ads_watch_event<E: Executor>(
    context: &Context<'_, E>,
    event: RawEvent<AdsWatch>,
) -> anyhow::Result<()> {
    let db: Db = context.extract();
    let Some(watch) = ads_watch_projection()
        .load(&event.aggregate_id)
        .execute(context.executor)
        .await?
    else {
        return Ok(());
    };
    db.write(&watch, |tx, watch| {
        Box::pin(async move {
            execute(
                tx,
                "INSERT INTO google_ads_watch (tenant_id, customer_id, next_at)
                 VALUES (?1, ?2, ?3)
                 ON CONFLICT (tenant_id) DO UPDATE SET
                    customer_id = excluded.customer_id, next_at = excluded.next_at",
                (
                    watch.tenant_id.as_str(),
                    watch.customer_id.as_str(),
                    watch.next_at as i64,
                ),
            )
            .await?;
            // The watch holds its own tally of the day: written whole, so a
            // repeated delivery counts nothing twice.
            execute(
                tx,
                "INSERT INTO google_ads_calls (day, tenant_id, calls) VALUES (?1, ?2, ?3)
                 ON CONFLICT (day, tenant_id) DO UPDATE SET calls = excluded.calls",
                (
                    watch.calls_day.as_str(),
                    watch.tenant_id.as_str(),
                    i64::from(watch.calls),
                ),
            )
            .await?;
            execute(
                tx,
                "DELETE FROM google_ads_calls WHERE day < ?1",
                (day_before(&watch.calls_day, KEPT_CALL_DAYS),),
            )
            .await?;
            Ok(())
        })
    })
    .await
}

/// The five figures of a row, in the tables' order.
fn figures(numbers: &AdsNumbers) -> [Value; 5] {
    [
        Value::from(numbers.cost_micros as i64),
        Value::from(numbers.impressions as i64),
        Value::from(numbers.clicks as i64),
        Value::from(numbers.conversions_milli as i64),
        Value::from(numbers.value_micros as i64),
    ]
}

/// Writes a day's rows of one kind, a few statements for the lot. `keys`
/// names the columns that tell a row from another on its day, and each row
/// gives their values, then its figures.
async fn insert_ads_rows(
    tx: &timada_core::db::Connection,
    table: &str,
    keys: &[&str],
    day: (&str, &str, &str),
    rows: Vec<(Vec<Value>, AdsNumbers)>,
) -> anyhow::Result<()> {
    let width = 3 + keys.len() + 5;
    for chunk in rows.chunks(ROWS_PER_INSERT) {
        let mut values = Vec::with_capacity(chunk.len() * width);
        let mut tuples = Vec::with_capacity(chunk.len());
        for (at, (named, numbers)) in chunk.iter().enumerate() {
            let places: Vec<String> = (1..=width)
                .map(|place| format!("?{}", at * width + place))
                .collect();
            tuples.push(format!("({})", places.join(", ")));
            values.extend([Value::from(day.0), Value::from(day.1), Value::from(day.2)]);
            values.extend(named.iter().cloned());
            values.extend(figures(numbers));
        }
        let statement = format!(
            "INSERT OR REPLACE INTO {table}
                (tenant_id, customer_id, day, {}, cost_micros, impressions, clicks,
                 conversions_milli, value_micros)
             VALUES {}",
            keys.join(", "),
            tuples.join(", ")
        );
        execute(tx, &statement, params_from_iter(values)).await?;
    }
    Ok(())
}

#[evento::subscription]
async fn on_ads_day_fetched<E: Executor>(
    context: &Context<'_, E>,
    event: Event<AdsDayFetched>,
) -> anyhow::Result<()> {
    let db: Db = context.extract();
    db.write(&event.data, |tx, day| {
        Box::pin(async move {
            let key = (
                day.tenant_id.as_str(),
                day.customer_id.as_str(),
                day.day.as_str(),
            );
            // The day's totals: every campaign on every device.
            let mut total = AdsNumbers::default();
            for row in &day.campaigns {
                total.cost_micros += row.numbers.cost_micros;
                total.impressions += row.numbers.impressions;
                total.clicks += row.numbers.clicks;
                total.conversions_milli += row.numbers.conversions_milli;
                total.value_micros += row.numbers.value_micros;
            }
            let mut values = vec![
                Value::from(key.0),
                Value::from(key.1),
                Value::from(key.2),
                Value::from(day.currency.as_str()),
            ];
            values.extend(figures(&total));
            execute(
                tx,
                "INSERT OR REPLACE INTO google_ads_days
                    (tenant_id, customer_id, day, currency, cost_micros, impressions, clicks,
                     conversions_milli, value_micros)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params_from_iter(values),
            )
            .await?;
            // A day read again replaces what it read before, whole.
            for table in [
                "google_ads_campaigns",
                "google_ads_ad_groups",
                "google_ads_products",
                "google_ads_terms",
            ] {
                execute(
                    tx,
                    &format!(
                        "DELETE FROM {table}
                         WHERE tenant_id = ?1 AND customer_id = ?2 AND day = ?3"
                    ),
                    key,
                )
                .await?;
            }
            let text = |value: &String| Value::from(value.as_str());
            insert_ads_rows(
                tx,
                "google_ads_campaigns",
                &["campaign_id", "device", "name"],
                key,
                day.campaigns
                    .iter()
                    .map(|row| {
                        (
                            vec![text(&row.campaign_id), text(&row.device), text(&row.name)],
                            row.numbers,
                        )
                    })
                    .collect(),
            )
            .await?;
            insert_ads_rows(
                tx,
                "google_ads_ad_groups",
                &["ad_group_id", "campaign_id", "name"],
                key,
                day.ad_groups
                    .iter()
                    .map(|row| {
                        (
                            vec![
                                text(&row.ad_group_id),
                                text(&row.campaign_id),
                                text(&row.name),
                            ],
                            row.numbers,
                        )
                    })
                    .collect(),
            )
            .await?;
            insert_ads_rows(
                tx,
                "google_ads_products",
                &["item_id", "ad_group_id", "campaign_id"],
                key,
                day.products
                    .iter()
                    .map(|row| {
                        (
                            vec![
                                text(&row.item_id),
                                text(&row.ad_group_id),
                                text(&row.campaign_id),
                            ],
                            row.numbers,
                        )
                    })
                    .collect(),
            )
            .await?;
            insert_ads_rows(
                tx,
                "google_ads_terms",
                &["term", "ad_group_id", "campaign_id"],
                key,
                day.terms
                    .iter()
                    .map(|row| {
                        (
                            vec![
                                text(&row.term),
                                text(&row.ad_group_id),
                                text(&row.campaign_id),
                            ],
                            row.numbers,
                        )
                    })
                    .collect(),
            )
            .await?;
            // The history kept, counted back from the day just read.
            for (table, kept) in [
                ("google_ads_days", KEPT_DAYS),
                ("google_ads_campaigns", KEPT_DAYS),
                ("google_ads_ad_groups", KEPT_DAYS),
                ("google_ads_products", KEPT_ROW_DAYS),
                ("google_ads_terms", KEPT_ROW_DAYS),
            ] {
                execute(
                    tx,
                    &format!(
                        "DELETE FROM {table}
                         WHERE tenant_id = ?1 AND customer_id = ?2 AND day < ?3"
                    ),
                    (key.0, key.1, day_before(&day.day, kept)),
                )
                .await?;
            }
            Ok(())
        })
    })
    .await
}

#[evento::subscription_all]
async fn on_merchant_watch_event<E: Executor>(
    context: &Context<'_, E>,
    event: RawEvent<MerchantWatch>,
) -> anyhow::Result<()> {
    let db: Db = context.extract();
    let Some(watch) = merchant_watch_projection()
        .load(&event.aggregate_id)
        .execute(context.executor)
        .await?
    else {
        return Ok(());
    };
    db.execute(
        "INSERT INTO google_merchant_watch (tenant_id, account_id, next_at) VALUES (?1, ?2, ?3)
         ON CONFLICT (tenant_id) DO UPDATE SET
            account_id = excluded.account_id, next_at = excluded.next_at",
        (
            watch.tenant_id.as_str(),
            watch.account_id.as_str(),
            watch.next_at as i64,
        ),
    )
    .await?;
    Ok(())
}

/// Writes rows of `width` values each into `table`, a few statements for
/// the lot.
async fn insert_wide(
    tx: &timada_core::db::Connection,
    table: &str,
    columns: &str,
    width: usize,
    rows: Vec<Vec<Value>>,
) -> anyhow::Result<()> {
    for chunk in rows.chunks(ROWS_PER_INSERT) {
        let mut values = Vec::with_capacity(chunk.len() * width);
        let mut tuples = Vec::with_capacity(chunk.len());
        for (at, row) in chunk.iter().enumerate() {
            let places: Vec<String> = (1..=width)
                .map(|place| format!("?{}", at * width + place))
                .collect();
            tuples.push(format!("({})", places.join(", ")));
            values.extend(row.iter().cloned());
        }
        let statement = format!(
            "INSERT OR REPLACE INTO {table} ({columns}) VALUES {}",
            tuples.join(", ")
        );
        execute(tx, &statement, params_from_iter(values)).await?;
    }
    Ok(())
}

/// A read replaces what the one before said of the account, whole: every
/// product, every issue.
#[evento::subscription]
async fn on_merchant_read<E: Executor>(
    context: &Context<'_, E>,
    event: Event<MerchantRead>,
) -> anyhow::Result<()> {
    let db: Db = context.extract();
    let read_at = event.timestamp as i64;
    db.write(&event.data, |tx, read| {
        Box::pin(async move {
            let key = (read.tenant_id.as_str(), read.account_id.as_str());
            for table in [
                "google_merchant_products",
                "google_merchant_issues",
                "google_merchant_account_issues",
            ] {
                execute(
                    tx,
                    &format!("DELETE FROM {table} WHERE tenant_id = ?1 AND account_id = ?2"),
                    key,
                )
                .await?;
            }
            execute(
                tx,
                "INSERT OR REPLACE INTO google_merchant_feeds
                    (tenant_id, account_id, read_at, more)
                 VALUES (?1, ?2, ?3, ?4)",
                (key.0, key.1, read_at, i64::from(read.more)),
            )
            .await?;
            let text = |value: &String| Value::from(value.as_str());
            insert_wide(
                tx,
                "google_merchant_products",
                "tenant_id, account_id, item_id, rank, ads, listings",
                6,
                read.products
                    .iter()
                    .map(|row| {
                        vec![
                            Value::from(key.0),
                            Value::from(key.1),
                            text(&row.item_id),
                            Value::from(i64::from(row.rank())),
                            text(&row.ads),
                            text(&row.listings),
                        ]
                    })
                    .collect(),
            )
            .await?;
            insert_wide(
                tx,
                "google_merchant_issues",
                "tenant_id, account_id, item_id, n, code, severity, attribute, context,
                 description, detail, documentation",
                11,
                read.products
                    .iter()
                    .flat_map(|row| {
                        row.issues.iter().enumerate().map(move |(n, issue)| {
                            vec![
                                Value::from(key.0),
                                Value::from(key.1),
                                text(&row.item_id),
                                Value::from(n as i64),
                                text(&issue.code),
                                text(&issue.severity),
                                text(&issue.attribute),
                                text(&issue.context),
                                text(&issue.description),
                                text(&issue.detail),
                                text(&issue.documentation),
                            ]
                        })
                    })
                    .collect(),
            )
            .await?;
            insert_wide(
                tx,
                "google_merchant_account_issues",
                "tenant_id, account_id, n, title, severity, detail, documentation",
                7,
                read.account_issues
                    .iter()
                    .enumerate()
                    .map(|(n, issue)| {
                        vec![
                            Value::from(key.0),
                            Value::from(key.1),
                            Value::from(n as i64),
                            text(&issue.title),
                            text(&issue.severity),
                            text(&issue.detail),
                            text(&issue.documentation),
                        ]
                    })
                    .collect(),
            )
            .await?;
            Ok(())
        })
    })
    .await
}
