//! Asking the suppliers, over and over.
//!
//! evento has no timers, so what is due lives in SQL (`sourcing_poll`) and a
//! ticker works it — the shape `timada_mailer::run_delivery` and
//! `timada_payment::run_provider_refunds` already use. A pass **claims** its
//! rows in one `UPDATE … RETURNING` with a lease, so any number of workers
//! may run side by side.
//!
//! One supplier is called at most once a pass, in batches of its own
//! choosing ([`ConnectorLimits`]), because the thing that goes wrong at
//! scale is not the arithmetic — it is being throttled, or banned, for
//! asking twenty thousand questions at once.

use std::{
    collections::BTreeMap,
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use evento::Executor;
use sqlx::SqlitePool;
use timada_core::ShopCurrencies;
use timada_tax::{ExchangeRateSource, ExchangeRates};

use crate::{
    command::Command,
    connector::{ConnectorError, ConnectorTask, SupplierConnectors, SupplierItemRef},
    error::SourcingError,
    price::{ReviewReason, Verdict},
    price_review::raise_review,
};

/// How often, how hard, and how patiently the suppliers are asked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncPolicy {
    /// How often one product is asked about.
    pub every: Duration,
    /// How soon a product somebody has an open order for is asked about
    /// again. An absolute level is a snapshot, and the products that matter
    /// are the ones moving.
    pub every_reserved: Duration,
    /// Waits before each retry while a supplier is unavailable; past the
    /// last one the row is simply postponed a whole `every`.
    pub retry_delays: Vec<Duration>,
    pub lease: Duration,
    /// Rows claimed per pass.
    pub batch: u32,
    /// A level past this is not believed — a feed with a decimal error must
    /// not put twenty thousand phantom units on sale.
    pub max_plausible_available: u32,
}

impl Default for SyncPolicy {
    fn default() -> Self {
        Self {
            every: Duration::from_secs(6 * 60 * 60),
            every_reserved: Duration::from_secs(15 * 60),
            retry_delays: vec![
                Duration::from_secs(60),
                Duration::from_secs(5 * 60),
                Duration::from_secs(30 * 60),
                Duration::from_secs(2 * 60 * 60),
            ],
            lease: Duration::from_secs(5 * 60),
            batch: 200,
            max_plausible_available: 100_000,
        }
    }
}

impl SyncPolicy {
    /// No waiting between attempts: for tests, and for the admin's
    /// « Synchroniser maintenant ».
    pub fn without_delays() -> Self {
        Self {
            retry_delays: Vec::new(),
            ..Self::default()
        }
    }

    fn wait_after(&self, attempts: i64) -> Duration {
        let index = usize::try_from(attempts.max(0)).unwrap_or(usize::MAX);
        self.retry_delays.get(index).copied().unwrap_or(self.every)
    }
}

/// What one pass did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SyncPass {
    /// Products the suppliers were asked about.
    pub asked: u32,
    /// Selling prices the rule moved by itself.
    pub repriced: u32,
    /// Prices an operator has to look at.
    pub queued: u32,
    /// Stock levels that moved.
    pub restocked: u32,
    /// Rows put off: a supplier could not be reached, or would not answer
    /// about an item.
    pub postponed: u32,
    /// Rows held because the supplier asked to be left alone for a while.
    /// Being throttled costs a row no attempt: it is not its fault.
    pub held: u32,
    /// Rows a pass could not make sense of at all.
    pub failed: u32,
}

/// Puts a product in the queue, or brings its next turn forward.
pub async fn enqueue(
    db: &SqlitePool,
    product_id: &str,
    supplier_id: &str,
    due_at: i64,
) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO sourcing_poll (product_id, supplier_id, next_poll_at)
         VALUES (?1, ?2, ?3)
         ON CONFLICT (product_id) DO UPDATE SET
            supplier_id = excluded.supplier_id,
            next_poll_at = MIN(sourcing_poll.next_poll_at, excluded.next_poll_at)",
    )
    .bind(product_id)
    .bind(supplier_id)
    .bind(due_at)
    .execute(db)
    .await?;
    Ok(())
}

/// Brings a product's next turn forward, if it is in the queue at all. Used
/// when something happened to it that makes its level worth re-reading.
pub async fn hurry(db: &SqlitePool, product_id: &str, due_at: i64) -> sqlx::Result<bool> {
    let done = sqlx::query(
        "UPDATE sourcing_poll SET next_poll_at = ?2
         WHERE product_id = ?1 AND next_poll_at > ?2",
    )
    .bind(product_id)
    .bind(due_at)
    .execute(db)
    .await?;
    Ok(done.rows_affected() > 0)
}

pub async fn dequeue(db: &SqlitePool, product_id: &str) -> sqlx::Result<()> {
    sqlx::query("DELETE FROM sourcing_poll WHERE product_id = ?")
        .bind(product_id)
        .execute(db)
        .await?;
    Ok(())
}

/// Everything a supplier sources, due now — for the admin's « Synchroniser
/// maintenant ».
pub async fn hurry_supplier(db: &SqlitePool, supplier_id: &str, due_at: i64) -> sqlx::Result<u64> {
    let done = sqlx::query(
        "UPDATE sourcing_poll SET next_poll_at = ?2, attempts = 0, last_error = NULL
         WHERE supplier_id = ?1",
    )
    .bind(supplier_id)
    .bind(due_at)
    .execute(db)
    .await?;
    Ok(done.rows_affected())
}

/// Where a product stands in the queue: what the back office shows next to
/// « dernière synchro », and what a test asserts on.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct PollStatus {
    pub product_id: String,
    pub supplier_id: String,
    pub next_poll_at: i64,
    /// Failures in a row. Reset as soon as a pass gets an answer.
    pub attempts: i64,
    pub last_error: Option<String>,
    pub last_polled_at: Option<i64>,
}

pub async fn poll_status(db: &SqlitePool, product_id: &str) -> sqlx::Result<Option<PollStatus>> {
    sqlx::query_as(
        "SELECT product_id, supplier_id, next_poll_at, attempts, last_error, last_polled_at
         FROM sourcing_poll WHERE product_id = ?",
    )
    .bind(product_id)
    .fetch_optional(db)
    .await
}

#[derive(Debug, sqlx::FromRow)]
struct PollRow {
    product_id: String,
    supplier_id: String,
    attempts: i64,
}

/// One pass with the built-in policy.
pub async fn sync_offers<E: Executor>(
    executor: &E,
    db: &SqlitePool,
    connectors: &SupplierConnectors,
    rates: &dyn ExchangeRates,
    currencies: &ShopCurrencies,
) -> Result<SyncPass, SourcingError> {
    sync_offers_with(
        executor,
        db,
        connectors,
        rates,
        currencies,
        &SyncPolicy::default(),
    )
    .await
}

/// Asks every supplier with something due what it costs and how many it
/// holds, and hands each answer to [`Command::apply_offer`].
pub async fn sync_offers_with<E: Executor>(
    executor: &E,
    db: &SqlitePool,
    connectors: &SupplierConnectors,
    rates: &dyn ExchangeRates,
    currencies: &ShopCurrencies,
    policy: &SyncPolicy,
) -> Result<SyncPass, SourcingError> {
    let now = timada_core::time::now_unix_secs()? as i64;
    let worker = worker_id();
    let rows: Vec<PollRow> = sqlx::query_as(
        "UPDATE sourcing_poll
         SET claimed_by = ?1, claimed_until = ?2
         WHERE product_id IN (
            SELECT product_id FROM sourcing_poll
            WHERE next_poll_at <= ?3 AND (claimed_until IS NULL OR claimed_until < ?3)
            ORDER BY next_poll_at, product_id
            LIMIT ?4)
         RETURNING product_id, supplier_id, attempts",
    )
    .bind(&worker)
    .bind(now + policy.lease.as_secs() as i64)
    .bind(now)
    .bind(policy.batch)
    .fetch_all(db)
    .await?;

    let mut by_supplier: BTreeMap<String, Vec<PollRow>> = BTreeMap::new();
    for row in rows {
        by_supplier
            .entry(row.supplier_id.clone())
            .or_default()
            .push(row);
    }

    let mut pass = SyncPass::default();
    for (supplier_id, rows) in by_supplier {
        ask_supplier(
            executor,
            db,
            connectors,
            rates,
            currencies,
            policy,
            &worker,
            &supplier_id,
            rows,
            now,
            &mut pass,
        )
        .await?;
    }
    Ok(pass)
}

#[allow(clippy::too_many_arguments)]
async fn ask_supplier<E: Executor>(
    executor: &E,
    db: &SqlitePool,
    connectors: &SupplierConnectors,
    rates: &dyn ExchangeRates,
    currencies: &ShopCurrencies,
    policy: &SyncPolicy,
    worker: &str,
    supplier_id: &str,
    rows: Vec<PollRow>,
    now: i64,
    pass: &mut SyncPass,
) -> Result<(), SourcingError> {
    let cmd = Command::new(executor, db.clone());
    let Some(supplier) = cmd.load_supplier(supplier_id).await? else {
        // Its rows are meaningless without it.
        for row in &rows {
            dequeue(db, &row.product_id).await?;
        }
        return Ok(());
    };
    // A supplier nobody can answer for, or one that is not to be bought from
    // for now, simply waits: its links and its prices are left alone.
    let connector = connectors.of(&supplier.connector);
    let skip = supplier.suspended
        || connector.is_none_or(|connector| !connector.does(ConnectorTask::Offers));
    if skip {
        for row in &rows {
            release(
                db,
                &row.product_id,
                worker,
                now + policy.every.as_secs() as i64,
                None,
            )
            .await?;
        }
        return Ok(());
    }
    let Some(connector) = connector else {
        return Ok(());
    };

    // What the shop knows each product as on the supplier's side.
    let mut items = Vec::with_capacity(rows.len());
    for row in &rows {
        match cmd.load_sourced_product(&row.product_id).await? {
            Some(sourced) if sourced.active => items.push((
                row,
                SupplierItemRef::new(sourced.external_item_id, sourced.external_sku),
            )),
            // Sourced no more: it has no business in the queue.
            _ => dequeue(db, &row.product_id).await?,
        }
    }

    let limits = connector.limits();
    let batch = usize::try_from(limits.batch.max(1)).unwrap_or(usize::MAX);
    for (chunk_index, chunk) in items.chunks(batch).enumerate() {
        if chunk_index > 0 && !limits.min_interval.is_zero() {
            tokio::time::sleep(limits.min_interval).await;
        }
        let asked: Vec<SupplierItemRef> = chunk.iter().map(|(_, item)| item.clone()).collect();
        let answers = match connector.offers(&asked).await {
            Ok(answers) => answers,
            Err(ConnectorError::RateLimited { retry_after }) => {
                // Not the rows' fault, so it costs them no attempt.
                tracing::warn!(%supplier_id, retry_after, "supplier is throttling us");
                for (row, _) in chunk {
                    release(
                        db,
                        &row.product_id,
                        worker,
                        now + retry_after as i64,
                        Some("rate limited"),
                    )
                    .await?;
                    pass.held += 1;
                }
                continue;
            }
            Err(err) => {
                let wait = wait_for(&err, policy, 0);
                tracing::warn!(%supplier_id, %err, "supplier would not answer");
                for (row, _) in chunk {
                    postpone(db, &row.product_id, worker, now, wait, &err.to_string()).await?;
                    pass.postponed += 1;
                }
                continue;
            }
        };

        for (row, item) in chunk {
            pass.asked += 1;
            let Some(offer) = answers.iter().find(|offer| offer.item.matches(item)) else {
                // Left out of the answer: the supplier no longer lists it.
                // Not the worker's business to delist it — one API call is a
                // poor reason to take a product off sale — so it is said to
                // an operator and asked again later.
                gone(db, &cmd, row, supplier_id, now).await?;
                release(
                    db,
                    &row.product_id,
                    worker,
                    now + policy.every.as_secs() as i64,
                    Some("item not listed by the supplier"),
                )
                .await?;
                pass.postponed += 1;
                continue;
            };
            if offer.available > policy.max_plausible_available {
                tracing::warn!(
                    product_id = %row.product_id,
                    available = offer.available,
                    "a supplier's level too large to believe: left alone"
                );
                release(
                    db,
                    &row.product_id,
                    worker,
                    now + policy.every.as_secs() as i64,
                    Some("implausible level"),
                )
                .await?;
                pass.failed += 1;
                continue;
            }

            match cmd
                .apply_offer(&row.product_id, offer, rates, currencies.base(), now as u64)
                .await
            {
                Ok(applied) => {
                    if applied.published.is_some() {
                        pass.restocked += 1;
                    }
                    match applied.verdict {
                        Verdict::Apply(_) => pass.repriced += 1,
                        Verdict::Review { .. } => pass.queued += 1,
                        Verdict::Unchanged => {}
                    }
                    if !applied.stale_currencies.is_empty() {
                        pass.queued += 1;
                    }
                    release(
                        db,
                        &row.product_id,
                        worker,
                        now + policy.every.as_secs() as i64,
                        None,
                    )
                    .await?;
                }
                Err(err) => {
                    tracing::error!(product_id = %row.product_id, %err, "offer could not be read");
                    postpone(
                        db,
                        &row.product_id,
                        worker,
                        now,
                        policy.wait_after(row.attempts),
                        &err.to_string(),
                    )
                    .await?;
                    pass.failed += 1;
                }
            }
        }
    }
    Ok(())
}

/// Says that a supplier stopped listing something, so somebody notices.
async fn gone<E: Executor>(
    db: &SqlitePool,
    cmd: &Command<'_, E>,
    row: &PollRow,
    supplier_id: &str,
    now: i64,
) -> Result<(), SourcingError> {
    let price =
        timada_pricing::load_product_price(cmd.executor, timada_pricing::price_id(&row.product_id))
            .await?;
    let currency = price
        .as_ref()
        .map(|price| price.listed_currency().to_owned())
        .unwrap_or_else(|| timada_core::Money::EUR.to_owned());
    let nothing = timada_core::Money::zero(&currency);
    raise_review(
        db,
        &row.product_id,
        supplier_id,
        ReviewReason::NoListedPrice,
        price.as_ref().map(|price| &price.price_incl_tax),
        &nothing,
        &nothing,
        0,
        now,
    )
    .await?;
    Ok(())
}

fn wait_for(err: &ConnectorError, policy: &SyncPolicy, attempts: i64) -> Duration {
    match err {
        // It will keep saying no, so there is nothing to retry quickly for.
        ConnectorError::Refused(_) | ConnectorError::UnknownItem(_) => policy.every,
        _ => policy.wait_after(attempts),
    }
}

/// Puts a row back with its next turn set, keeping the attempt count where
/// it was: the pass got an answer.
async fn release(
    db: &SqlitePool,
    product_id: &str,
    worker: &str,
    next_poll_at: i64,
    last_error: Option<&str>,
) -> sqlx::Result<()> {
    sqlx::query(
        "UPDATE sourcing_poll
         SET next_poll_at = ?3, attempts = 0, last_error = ?4,
             last_polled_at = ?5, claimed_by = NULL, claimed_until = NULL
         WHERE product_id = ?1 AND claimed_by = ?2",
    )
    .bind(product_id)
    .bind(worker)
    .bind(next_poll_at)
    .bind(last_error)
    .bind(timada_core::time::now_unix_secs().unwrap_or_default() as i64)
    .execute(db)
    .await?;
    Ok(())
}

/// Puts a row back after a failure, one attempt the worse.
async fn postpone(
    db: &SqlitePool,
    product_id: &str,
    worker: &str,
    now: i64,
    wait: Duration,
    last_error: &str,
) -> sqlx::Result<()> {
    sqlx::query(
        "UPDATE sourcing_poll
         SET attempts = attempts + 1, next_poll_at = ?3, last_error = ?4,
             claimed_by = NULL, claimed_until = NULL
         WHERE product_id = ?1 AND claimed_by = ?2",
    )
    .bind(product_id)
    .bind(worker)
    .bind(now + wait.as_secs() as i64)
    .bind(last_error)
    .execute(db)
    .await?;
    Ok(())
}

fn worker_id() -> String {
    static PASSES: AtomicU64 = AtomicU64::new(0);
    format!(
        "{}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos()),
        PASSES.fetch_add(1, Ordering::Relaxed)
    )
}

/// Runs [`sync_offers_with`] every `every`, forever. Any number of these can
/// run: each pass claims its own rows.
pub async fn run_offer_sync<E: Executor>(
    executor: E,
    db: SqlitePool,
    connectors: SupplierConnectors,
    rates: ExchangeRateSource,
    currencies: ShopCurrencies,
    every: Duration,
) {
    let policy = SyncPolicy::default();
    let mut ticker = tokio::time::interval(every);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        ticker.tick().await;
        let pass = sync_offers_with(
            &executor,
            &db,
            &connectors,
            rates.0.as_ref(),
            &currencies,
            &policy,
        )
        .await;
        match pass {
            Ok(pass) if pass.asked > 0 => tracing::info!(?pass, "supplier sync pass"),
            Ok(_) => {}
            Err(err) => tracing::error!(error = %err, "supplier sync pass failed"),
        }
    }
}
