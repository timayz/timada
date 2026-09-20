//! Refunds on their way to the provider. The `payment-refund-execution`
//! subscription only *enqueues* each `RefundRequested` into
//! `payment_provider_refund`; [`execute_pending_refunds`] hands the rows to the
//! [`PaymentProvider`] and records the outcome in the payment's stream —
//! `PaymentRefunded` + `RefundSettled`, or `RefundFailed`. Trouble reaching
//! the provider stays in SQL and is retried; only its final word is an event.

use std::{
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use evento::{
    Executor,
    metadata::Event,
    subscription::{Context, SubscriptionBuilder},
};
use sqlx::SqlitePool;
use timada_core::Money;

use crate::{
    aggregator::RefundRequested,
    command::Command,
    error::PaymentError,
    provider::{PaymentProvider, ProviderError, ProviderRefund, RefundOutcome},
    query::load_payment,
    value_object::RefundStatus,
};

/// Subscription key; the caller attaches the pool with `.data(pool)`.
pub const REFUND_EXECUTION_SUBSCRIPTION: &str = "payment-refund-execution";

/// Not strict: a process manager, it only looks at the refunds asked for.
pub fn refund_execution_subscription<E: Executor>() -> SubscriptionBuilder<E> {
    SubscriptionBuilder::new(REFUND_EXECUTION_SUBSCRIPTION).handler(enqueue_on_refund_requested())
}

/// Keyed by the event id, so a redelivery enqueues nothing; a retried refund is
/// another event, hence another row — and another idempotency key.
#[evento::subscription]
async fn enqueue_on_refund_requested<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<RefundRequested>,
) -> anyhow::Result<()> {
    let db = ctx
        .get::<SqlitePool>()
        .ok_or_else(|| anyhow::anyhow!("SqlitePool missing from subscription context"))?;
    sqlx::query(
        "INSERT OR IGNORE INTO payment_provider_refund
            (request_id, refund_id, payment_id, amount_minor, currency, created_at)
         VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(event.id.to_string())
    .bind(&event.data.refund_id)
    .bind(&event.aggregate_id)
    .bind(event.data.amount.minor)
    .bind(&event.data.amount.currency)
    .bind(event.timestamp as i64)
    .execute(&db)
    .await?;
    Ok(())
}

/// How refunds are handed to the provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefundPolicy {
    /// How long to wait before each retry when the provider is unavailable, in
    /// order; past the last one the refund fails and waits for an operator.
    pub retry_delays: Vec<Duration>,
    /// How long a worker may hold the rows it claimed.
    pub lease: Duration,
    /// Rows claimed per pass.
    pub batch: u32,
}

impl Default for RefundPolicy {
    /// 1 min, 5 min, 30 min, 2 h; a five-minute lease; 50 rows a pass.
    fn default() -> Self {
        Self {
            retry_delays: [60, 300, 1_800, 7_200]
                .into_iter()
                .map(Duration::from_secs)
                .collect(),
            lease: Duration::from_secs(300),
            batch: 50,
        }
    }
}

impl RefundPolicy {
    /// Retries at once: for tests.
    pub fn without_delays() -> Self {
        let policy = Self::default();
        Self {
            retry_delays: vec![Duration::ZERO; policy.retry_delays.len()],
            ..policy
        }
    }

    fn max_attempts(&self) -> i64 {
        self.retry_delays.len() as i64 + 1
    }
}

/// What a pass did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RefundPass {
    /// The provider confirmed; `PaymentRefunded` is recorded.
    pub settled: u32,
    /// The provider took the refund and will report later.
    pub awaiting: u32,
    /// Refused, or given up on: `RefundFailed` is recorded.
    pub failed: u32,
    /// The provider was unavailable; the refund will be tried again.
    pub postponed: u32,
}

#[derive(sqlx::FromRow)]
struct WorkRow {
    request_id: String,
    refund_id: String,
    payment_id: String,
    amount_minor: i64,
    currency: String,
    attempts: i64,
}

/// A name no other pass shares: the rows a pass claims are its own.
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

async fn close(
    db: &SqlitePool,
    row: &WorkRow,
    reference: Option<&str>,
    error: Option<&str>,
) -> Result<(), PaymentError> {
    sqlx::query(
        "UPDATE payment_provider_refund
         SET done_at = ?, attempts = attempts + 1, provider_reference = ?, last_error = ?,
             claimed_by = NULL, claimed_until = NULL
         WHERE request_id = ?",
    )
    .bind(timada_core::time::now_unix_secs()? as i64)
    .bind(reference)
    .bind(error)
    .bind(&row.request_id)
    .execute(db)
    .await?;
    Ok(())
}

/// Hands the refunds that are due to the provider, oldest first. The pass
/// **claims** its rows in one `UPDATE … RETURNING`, so any number of workers
/// can run side by side; the row's id is the provider's idempotency key, so
/// even a worker that died after the provider answered refunds nothing twice.
pub async fn execute_pending_refunds<E: Executor>(
    executor: &E,
    db: &SqlitePool,
    provider: &dyn PaymentProvider,
    policy: &RefundPolicy,
) -> Result<RefundPass, PaymentError> {
    let now = timada_core::time::now_unix_secs()? as i64;
    let worker = worker_id();
    let rows: Vec<WorkRow> = sqlx::query_as(
        "UPDATE payment_provider_refund
         SET claimed_by = ?1, claimed_until = ?2
         WHERE request_id IN (
            SELECT request_id FROM payment_provider_refund
            WHERE done_at IS NULL AND next_attempt_at <= ?3
              AND (claimed_until IS NULL OR claimed_until < ?3)
            ORDER BY created_at, request_id
            LIMIT ?4)
         RETURNING request_id, refund_id, payment_id, amount_minor, currency, attempts",
    )
    .bind(&worker)
    .bind(now + policy.lease.as_secs() as i64)
    .bind(now)
    .bind(policy.batch)
    .fetch_all(db)
    .await?;

    let cmd = Command(executor);
    let mut pass = RefundPass::default();
    for row in rows {
        let payment = load_payment(executor, &row.payment_id)
            .await?
            .ok_or(PaymentError::PaymentNotFound)?;
        let still_pending = payment
            .refunds
            .iter()
            .any(|r| r.refund_id == row.refund_id && r.status == RefundStatus::Pending);
        if !still_pending {
            // Settled by hand or failed in the meantime: nothing to send.
            close(db, &row, None, None).await?;
            continue;
        }
        let Some(psp_reference) = payment.psp_reference.clone() else {
            cmd.fail_refund(
                &row.payment_id,
                &row.refund_id,
                "payment has no provider reference".to_owned(),
            )
            .await?;
            close(db, &row, None, Some("no provider reference")).await?;
            pass.failed += 1;
            continue;
        };

        let refund = ProviderRefund {
            psp_reference,
            amount: Money::new(row.amount_minor, &row.currency),
            idempotency_key: row.request_id.clone(),
        };
        match provider.refund(&refund).await {
            Ok(RefundOutcome::Settled { reference }) => {
                cmd.settle_refund(&row.payment_id, &row.refund_id, reference.clone())
                    .await?;
                close(db, &row, Some(&reference), None).await?;
                pass.settled += 1;
            }
            Ok(RefundOutcome::Pending { reference }) => {
                close(db, &row, Some(&reference), None).await?;
                pass.awaiting += 1;
            }
            Err(ProviderError::Refused(reason)) => {
                cmd.fail_refund(&row.payment_id, &row.refund_id, reason.clone())
                    .await?;
                close(db, &row, None, Some(&reason)).await?;
                pass.failed += 1;
            }
            Err(ProviderError::Unavailable(error)) => {
                tracing::warn!(refund_id = %row.refund_id, %error, "provider unavailable for a refund");
                // `attempts` is the count before this one: it indexes the wait
                // before the next. Past the schedule the refund is given up on.
                let Some(wait) = policy.retry_delays.get(row.attempts.max(0) as usize) else {
                    let reason = format!(
                        "provider unavailable after {} attempts: {error}",
                        policy.max_attempts()
                    );
                    cmd.fail_refund(&row.payment_id, &row.refund_id, reason.clone())
                        .await?;
                    close(db, &row, None, Some(&reason)).await?;
                    pass.failed += 1;
                    continue;
                };
                sqlx::query(
                    "UPDATE payment_provider_refund
                     SET attempts = attempts + 1, last_error = ?, next_attempt_at = ?,
                         claimed_by = NULL, claimed_until = NULL
                     WHERE request_id = ? AND claimed_by = ?",
                )
                .bind(&error)
                .bind(timada_core::time::now_unix_secs()? as i64 + wait.as_secs() as i64)
                .bind(&row.request_id)
                .bind(&worker)
                .execute(db)
                .await?;
                pass.postponed += 1;
            }
        }
    }
    Ok(pass)
}

/// Runs [`execute_pending_refunds`] every `every`, forever. Any number of these
/// can run: each pass claims its own rows.
pub async fn run_provider_refunds<E: Executor>(
    executor: E,
    db: SqlitePool,
    provider: Arc<dyn PaymentProvider>,
    every: Duration,
) {
    let policy = RefundPolicy::default();
    let mut ticker = tokio::time::interval(every);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        ticker.tick().await;
        if let Err(err) = execute_pending_refunds(&executor, &db, provider.as_ref(), &policy).await
        {
            tracing::error!(error = %err, "refund pass failed");
        }
    }
}

/// The refund a provider reference belongs to: `(payment_id, refund_id)`.
pub(crate) async fn refund_by_provider_reference(
    db: &SqlitePool,
    reference: &str,
) -> sqlx::Result<Option<(String, String)>> {
    sqlx::query_as(
        "SELECT payment_id, refund_id FROM payment_provider_refund
         WHERE provider_reference = ? ORDER BY created_at DESC LIMIT 1",
    )
    .bind(reference)
    .fetch_optional(db)
    .await
}
