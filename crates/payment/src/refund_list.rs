//! SQL list read model behind the admin's refunds section: one row per
//! `PaymentRefunded` event. Fed by the `payment-refund-list` subscription.

use evento::{
    Executor,
    metadata::Event,
    subscription::{Context, SubscriptionBuilder},
};
use sqlx::SqlitePool;

use crate::{
    aggregator::{PaymentCaptured, PaymentDeclined, PaymentRefunded, PaymentRequested},
    query::load_payment,
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
