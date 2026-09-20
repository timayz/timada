//! SQL list read models behind the admin's refunds section, both fed by the
//! `payment-refund-list` subscription: `payment_refund_list`, one row per
//! `PaymentRefunded` event (money that went back), and
//! `payment_refund_request_list`, one row per refund asked for, with what
//! became of it.

use evento::{
    Executor,
    metadata::Event,
    subscription::{Context, SubscriptionBuilder},
};
use sqlx::SqlitePool;

use crate::{
    aggregator::{
        PaymentCaptured, PaymentDeclined, PaymentRefunded, PaymentRequested, RefundFailed,
        RefundRequested, RefundSettled,
    },
    query::load_payment,
    value_object::RefundStatus,
};

/// Subscription key; the caller attaches the pool with `.data(pool)`.
pub const REFUND_LIST_SUBSCRIPTION: &str = "payment-refund-list";

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct RefundListRow {
    /// The id of the `PaymentRefunded` event: a payment can be refunded in
    /// several goes.
    pub refund_id: String,
    pub payment_id: String,
    pub order_id: String,
    pub amount_minor: i64,
    pub currency: String,
    pub reason: String,
    pub refunded_at: i64,
}

pub fn refund_list_subscription<E: Executor>() -> SubscriptionBuilder<E> {
    SubscriptionBuilder::new(REFUND_LIST_SUBSCRIPTION)
        .handler(insert_on_payment_refunded())
        .handler(upsert_on_refund_requested())
        .handler(update_on_refund_settled())
        .handler(update_on_refund_failed())
        .skip::<PaymentRequested>()
        .skip::<PaymentCaptured>()
        .skip::<PaymentDeclined>()
        .strict()
}

/// Refunds across all orders, newest first.
pub async fn list_refunds(
    db: &SqlitePool,
    limit: u32,
    offset: u32,
) -> sqlx::Result<Vec<RefundListRow>> {
    sqlx::query_as(
        "SELECT refund_id, payment_id, order_id, amount_minor, currency, reason, refunded_at
         FROM payment_refund_list
         ORDER BY refunded_at DESC, refund_id DESC
         LIMIT ?1 OFFSET ?2",
    )
    .bind(limit)
    .bind(offset)
    .fetch_all(db)
    .await
}

pub async fn count_refunds(db: &SqlitePool) -> sqlx::Result<i64> {
    sqlx::query_scalar("SELECT COUNT(*) FROM payment_refund_list")
        .fetch_one(db)
        .await
}

/// Keyed by the event id, so a redelivery inserts nothing.
#[evento::subscription]
async fn insert_on_payment_refunded<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<PaymentRefunded>,
) -> anyhow::Result<()> {
    let db = ctx
        .get::<SqlitePool>()
        .ok_or_else(|| anyhow::anyhow!("SqlitePool missing from subscription context"))?;
    let Some(payment) = load_payment(ctx.executor, &event.aggregate_id).await? else {
        anyhow::bail!(
            "payment {} refunded but cannot be loaded",
            event.aggregate_id
        );
    };
    sqlx::query(
        "INSERT OR IGNORE INTO payment_refund_list
            (refund_id, payment_id, order_id, amount_minor, currency, reason, refunded_at)
         VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(event.id.to_string())
    .bind(&event.aggregate_id)
    .bind(&payment.order_id)
    .bind(event.data.amount.minor)
    .bind(&event.data.amount.currency)
    .bind(&event.data.reason)
    .bind(event.timestamp as i64)
    .execute(&db)
    .await?;
    Ok(())
}

/// A refund that was asked for, as the admin lists it.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct RefundRequestRow {
    pub refund_id: String,
    pub payment_id: String,
    pub order_id: String,
    pub amount_minor: i64,
    pub currency: String,
    pub reason: String,
    /// [`RefundStatus::as_str`].
    pub status: String,
    pub failure: Option<String>,
    pub psp_refund_reference: Option<String>,
    pub requested_at: i64,
    pub updated_at: i64,
}

/// Refunds that were asked for, newest first; `status` narrows the list.
pub async fn list_refund_requests(
    db: &SqlitePool,
    status: Option<RefundStatus>,
    limit: u32,
    offset: u32,
) -> sqlx::Result<Vec<RefundRequestRow>> {
    sqlx::query_as(
        "SELECT refund_id, payment_id, order_id, amount_minor, currency, reason, status,
                failure, psp_refund_reference, requested_at, updated_at
         FROM payment_refund_request_list
         WHERE ?1 IS NULL OR status = ?1
         ORDER BY requested_at DESC, refund_id DESC
         LIMIT ?2 OFFSET ?3",
    )
    .bind(status.map(RefundStatus::as_str))
    .bind(limit)
    .bind(offset)
    .fetch_all(db)
    .await
}

pub async fn count_refund_requests(
    db: &SqlitePool,
    status: Option<RefundStatus>,
) -> sqlx::Result<i64> {
    sqlx::query_scalar(
        "SELECT COUNT(*) FROM payment_refund_request_list WHERE ?1 IS NULL OR status = ?1",
    )
    .bind(status.map(RefundStatus::as_str))
    .fetch_one(db)
    .await
}

fn pool<E: Executor>(ctx: &Context<'_, E>) -> anyhow::Result<SqlitePool> {
    ctx.get::<SqlitePool>()
        .ok_or_else(|| anyhow::anyhow!("SqlitePool missing from subscription context"))
}

/// Rewrites a refund's row with where the refund stands *now* in the payment's
/// stream — absolute values, so a redelivery changes nothing.
async fn refresh_request<E: Executor>(
    ctx: &Context<'_, E>,
    payment_id: &str,
    refund_id: &str,
    at: u64,
) -> anyhow::Result<()> {
    let db = pool(ctx)?;
    let Some(payment) = load_payment(ctx.executor, payment_id).await? else {
        anyhow::bail!("payment {payment_id} has a refund but cannot be loaded");
    };
    let Some(refund) = payment.refunds.iter().find(|r| r.refund_id == refund_id) else {
        anyhow::bail!("payment {payment_id} does not know its refund {refund_id}");
    };
    sqlx::query(
        "INSERT INTO payment_refund_request_list
            (refund_id, payment_id, order_id, amount_minor, currency, reason, status,
             failure, psp_refund_reference, requested_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
         ON CONFLICT (refund_id) DO UPDATE
         SET status = excluded.status, failure = excluded.failure,
             psp_refund_reference = excluded.psp_refund_reference,
             updated_at = MAX(updated_at, excluded.updated_at)",
    )
    .bind(&refund.refund_id)
    .bind(&payment.id)
    .bind(&payment.order_id)
    .bind(refund.amount.minor)
    .bind(&refund.amount.currency)
    .bind(&refund.reason)
    .bind(refund.status.as_str())
    .bind(&refund.failure)
    .bind(&refund.psp_refund_reference)
    .bind(refund.requested_at as i64)
    .bind(at as i64)
    .execute(&db)
    .await?;
    Ok(())
}

#[evento::subscription]
async fn upsert_on_refund_requested<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<RefundRequested>,
) -> anyhow::Result<()> {
    refresh_request(
        ctx,
        &event.aggregate_id,
        &event.data.refund_id,
        event.timestamp,
    )
    .await
}

#[evento::subscription]
async fn update_on_refund_settled<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<RefundSettled>,
) -> anyhow::Result<()> {
    refresh_request(
        ctx,
        &event.aggregate_id,
        &event.data.refund_id,
        event.timestamp,
    )
    .await
}

#[evento::subscription]
async fn update_on_refund_failed<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<RefundFailed>,
) -> anyhow::Result<()> {
    refresh_request(
        ctx,
        &event.aggregate_id,
        &event.data.refund_id,
        event.timestamp,
    )
    .await
}
