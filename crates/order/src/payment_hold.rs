//! Orders held because their payment is disputed. A cardholder who contests
//! a charge may get the money back from their bank: shipping the goods
//! meanwhile is how a shop loses twice. `order_payment_hold` lists the orders
//! concerned, fed by the `order-payment-hold` subscription from the payment
//! context's dispute events; the queue of orders to ship leaves them out.

use evento::{
    Executor,
    metadata::Event,
    subscription::{Context, SubscriptionBuilder},
};
use sqlx::SqlitePool;
use timada_payment::aggregator::{DisputeLost, DisputeOpened, DisputeWon};

/// Subscription key; the caller attaches the pool with `.data(pool)`.
pub const PAYMENT_HOLD_SUBSCRIPTION: &str = "order-payment-hold";

/// Not strict: an anti-corruption layer over the payment's disputes.
pub fn payment_hold_subscription<E: Executor>() -> SubscriptionBuilder<E> {
    SubscriptionBuilder::new(PAYMENT_HOLD_SUBSCRIPTION)
        .handler(hold_on_dispute_opened())
        .handler(release_on_dispute_won())
        .handler(release_on_dispute_lost())
}

/// Whether the order is held for a dispute on its payment.
pub async fn is_order_on_hold(db: &SqlitePool, order_id: &str) -> sqlx::Result<bool> {
    sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM order_payment_hold WHERE order_id = ?)")
        .bind(order_id)
        .fetch_one(db)
        .await
}

/// How many paid orders are kept out of the queue of orders to ship.
pub async fn count_orders_on_hold(db: &SqlitePool) -> sqlx::Result<i64> {
    sqlx::query_scalar(
        "SELECT COUNT(*) FROM order_history
         WHERE status = 'paid' AND order_id IN (SELECT order_id FROM order_payment_hold)",
    )
    .fetch_one(db)
    .await
}

/// Rewrites the order's hold from where its payment stands *now*: held while
/// any dispute is open. Absolute, so a redelivery changes nothing — and a
/// lost dispute releases the hold too: what to do with the order then is the
/// operator's call, not a queue's.
async fn refresh<E: Executor>(
    ctx: &Context<'_, E>,
    payment_id: &str,
    at: u64,
) -> anyhow::Result<()> {
    let db = ctx
        .get::<SqlitePool>()
        .ok_or_else(|| anyhow::anyhow!("SqlitePool missing from subscription context"))?;
    let Some(payment) = timada_payment::load_payment(ctx.executor, payment_id).await? else {
        anyhow::bail!("payment {payment_id} disputed but cannot be loaded");
    };
    if payment.open_dispute().is_some() {
        sqlx::query("INSERT OR IGNORE INTO order_payment_hold (order_id, since) VALUES (?, ?)")
            .bind(&payment.order_id)
            .bind(at as i64)
            .execute(&db)
            .await?;
    } else {
        sqlx::query("DELETE FROM order_payment_hold WHERE order_id = ?")
            .bind(&payment.order_id)
            .execute(&db)
            .await?;
    }
    Ok(())
}

#[evento::subscription]
async fn hold_on_dispute_opened<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<DisputeOpened>,
) -> anyhow::Result<()> {
    refresh(ctx, &event.aggregate_id, event.timestamp).await
}

#[evento::subscription]
async fn release_on_dispute_won<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<DisputeWon>,
) -> anyhow::Result<()> {
    refresh(ctx, &event.aggregate_id, event.timestamp).await
}

#[evento::subscription]
async fn release_on_dispute_lost<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<DisputeLost>,
) -> anyhow::Result<()> {
    refresh(ctx, &event.aggregate_id, event.timestamp).await
}
