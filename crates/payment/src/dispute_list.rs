//! SQL read models behind the admin's disputes queue, fed by the
//! `payment-dispute-list` subscription: `payment_dispute_list`, one row per
//! dispute with where it stands, and `payment_capture_reference`, which says
//! what payment a provider's reference belongs to — a provider reports a
//! dispute against *its* reference.

use evento::{
    Executor,
    metadata::Event,
    subscription::{Context, SubscriptionBuilder},
};
use sqlx::SqlitePool;

use crate::{
    aggregator::{
        DisputeLost, DisputeOpened, DisputeWon, PaymentCaptured, PaymentDeclined, PaymentRefunded,
        PaymentRequested, RefundFailed, RefundRequested, RefundSettled,
    },
    query::load_payment,
    value_object::DisputeStatus,
};

/// Subscription key; the caller attaches the pool with `.data(pool)`.
pub const DISPUTE_LIST_SUBSCRIPTION: &str = "payment-dispute-list";

pub fn dispute_list_subscription<E: Executor>() -> SubscriptionBuilder<E> {
    SubscriptionBuilder::new(DISPUTE_LIST_SUBSCRIPTION)
        .handler(remember_on_payment_captured())
        .handler(refresh_on_dispute_opened())
        .handler(refresh_on_dispute_won())
        .handler(refresh_on_dispute_lost())
        .skip::<PaymentRequested>()
        .skip::<PaymentDeclined>()
        .skip::<PaymentRefunded>()
        .skip::<RefundRequested>()
        .skip::<RefundSettled>()
        .skip::<RefundFailed>()
        .strict()
}

/// A dispute as the admin lists it.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct DisputeRow {
    /// The provider's own reference.
    pub dispute_id: String,
    pub payment_id: String,
    pub order_id: String,
    pub amount_minor: i64,
    pub currency: String,
    /// The provider's reason code: see [`crate::dispute_reason_label`].
    pub reason: String,
    /// [`DisputeStatus::as_str`].
    pub status: String,
    /// Unix seconds: when the shop's evidence is due.
    pub respond_by: Option<i64>,
    pub opened_at: i64,
    pub closed_at: Option<i64>,
}

/// Disputes, the most pressing first: open ones by their deadline, then the
/// closed ones, newest first. `status` narrows the list.
pub async fn list_disputes(
    db: &SqlitePool,
    status: Option<DisputeStatus>,
    limit: u32,
    offset: u32,
) -> sqlx::Result<Vec<DisputeRow>> {
    sqlx::query_as(
        "SELECT dispute_id, payment_id, order_id, amount_minor, currency, reason, status,
                respond_by, opened_at, closed_at
         FROM payment_dispute_list
         WHERE ?1 IS NULL OR status = ?1
         ORDER BY status = 'open' DESC,
                  CASE WHEN status = 'open' THEN COALESCE(respond_by, opened_at) END,
                  opened_at DESC, dispute_id
         LIMIT ?2 OFFSET ?3",
    )
    .bind(status.map(DisputeStatus::as_str))
    .bind(limit)
    .bind(offset)
    .fetch_all(db)
    .await
}

pub async fn count_disputes(db: &SqlitePool, status: Option<DisputeStatus>) -> sqlx::Result<i64> {
    sqlx::query_scalar("SELECT COUNT(*) FROM payment_dispute_list WHERE ?1 IS NULL OR status = ?1")
        .bind(status.map(DisputeStatus::as_str))
        .fetch_one(db)
        .await
}

/// The disputes of one order, oldest first.
pub async fn disputes_of_order(db: &SqlitePool, order_id: &str) -> sqlx::Result<Vec<DisputeRow>> {
    sqlx::query_as(
        "SELECT dispute_id, payment_id, order_id, amount_minor, currency, reason, status,
                respond_by, opened_at, closed_at
         FROM payment_dispute_list WHERE order_id = ? ORDER BY opened_at, dispute_id",
    )
    .bind(order_id)
    .fetch_all(db)
    .await
}

/// The orders, among `order_ids`, whose payment has a dispute the bank has not
/// decided: they are not to be shipped.
pub async fn orders_with_open_dispute(
    db: &SqlitePool,
    order_ids: &[String],
) -> sqlx::Result<Vec<String>> {
    if order_ids.is_empty() {
        return Ok(Vec::new());
    }
    let mut query = sqlx::QueryBuilder::<sqlx::Sqlite>::new(
        "SELECT DISTINCT order_id FROM payment_dispute_list
         WHERE status = 'open' AND order_id IN (",
    );
    let mut bound = query.separated(", ");
    for id in order_ids {
        bound.push_bind(id);
    }
    query.push(")");
    query.build_query_scalar().fetch_all(db).await
}

/// The payment a provider's reference was captured for.
pub async fn payment_by_reference(
    db: &SqlitePool,
    psp_reference: &str,
) -> sqlx::Result<Option<String>> {
    sqlx::query_scalar("SELECT payment_id FROM payment_capture_reference WHERE psp_reference = ?")
        .bind(psp_reference)
        .fetch_optional(db)
        .await
}

fn pool<E: Executor>(ctx: &Context<'_, E>) -> anyhow::Result<SqlitePool> {
    ctx.get::<SqlitePool>()
        .ok_or_else(|| anyhow::anyhow!("SqlitePool missing from subscription context"))
}

#[evento::subscription]
async fn remember_on_payment_captured<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<PaymentCaptured>,
) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO payment_capture_reference (psp_reference, payment_id) VALUES (?, ?)
         ON CONFLICT (psp_reference) DO UPDATE SET payment_id = excluded.payment_id",
    )
    .bind(&event.data.psp_reference)
    .bind(&event.aggregate_id)
    .execute(&pool(ctx)?)
    .await?;
    Ok(())
}

/// Rewrites a dispute's row with where it stands *now* in the payment's
/// stream — absolute values, so a redelivery changes nothing.
async fn refresh<E: Executor>(
    ctx: &Context<'_, E>,
    payment_id: &str,
    dispute_id: &str,
) -> anyhow::Result<()> {
    let Some(payment) = load_payment(ctx.executor, payment_id).await? else {
        anyhow::bail!("payment {payment_id} disputed but cannot be loaded");
    };
    let Some(dispute) = payment.disputes.iter().find(|d| d.dispute_id == dispute_id) else {
        anyhow::bail!("payment {payment_id} has no dispute {dispute_id}");
    };
    sqlx::query(
        "INSERT INTO payment_dispute_list
            (dispute_id, payment_id, order_id, amount_minor, currency, reason, status,
             respond_by, opened_at, closed_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
         ON CONFLICT (dispute_id) DO UPDATE SET
            status = excluded.status, respond_by = excluded.respond_by,
            closed_at = excluded.closed_at",
    )
    .bind(dispute_id)
    .bind(payment_id)
    .bind(&payment.order_id)
    .bind(dispute.amount.minor)
    .bind(&dispute.amount.currency)
    .bind(&dispute.reason)
    .bind(dispute.status.as_str())
    .bind(dispute.respond_by.map(|at| at as i64))
    .bind(dispute.opened_at as i64)
    .bind(dispute.closed_at.map(|at| at as i64))
    .execute(&pool(ctx)?)
    .await?;
    Ok(())
}

#[evento::subscription]
async fn refresh_on_dispute_opened<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<DisputeOpened>,
) -> anyhow::Result<()> {
    refresh(ctx, &event.aggregate_id, &event.data.dispute_id).await
}

#[evento::subscription]
async fn refresh_on_dispute_won<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<DisputeWon>,
) -> anyhow::Result<()> {
    refresh(ctx, &event.aggregate_id, &event.data.dispute_id).await
}

#[evento::subscription]
async fn refresh_on_dispute_lost<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<DisputeLost>,
) -> anyhow::Result<()> {
    refresh(ctx, &event.aggregate_id, &event.data.dispute_id).await
}
