//! Keeping the queue of what to ask about in step with what is sourced —
//! and bringing a product's turn forward when something happens to it that
//! makes its level worth re-reading.

use evento::{
    Executor,
    metadata::Event,
    subscription::{Context, SubscriptionBuilder},
};
use sqlx::SqlitePool;
use timada_inventory::aggregator::{StockReservationReleased, StockReserved};

use crate::{
    aggregator::{ProductSourced, SourcingStopped},
    command::Command,
    sync::{SyncPolicy, dequeue, enqueue, hurry},
};

/// Subscription key; the caller attaches the pool with `.data(pool)`, and
/// optionally a [`SyncPolicy`].
pub const SOURCING_POLL_SUBSCRIPTION: &str = "sourcing-poll";

/// Not strict: it watches a subset of two contexts' events.
pub fn sourcing_poll_subscription<E: Executor>() -> SubscriptionBuilder<E> {
    SubscriptionBuilder::new(SOURCING_POLL_SUBSCRIPTION)
        .handler(enqueue_on_product_sourced())
        .handler(dequeue_on_sourcing_stopped())
        .handler(hurry_on_stock_reserved())
        .handler(hurry_on_stock_released())
}

fn pool<E: Executor>(ctx: &Context<'_, E>) -> anyhow::Result<SqlitePool> {
    ctx.get::<SqlitePool>()
        .ok_or_else(|| anyhow::anyhow!("SqlitePool missing from subscription context"))
}

fn policy<E: Executor>(ctx: &Context<'_, E>) -> SyncPolicy {
    ctx.get::<SyncPolicy>().unwrap_or_default()
}

/// A newly sourced product is asked about at once: the shop has no idea yet
/// what it costs or whether the supplier holds any.
#[evento::subscription]
async fn enqueue_on_product_sourced<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<ProductSourced>,
) -> anyhow::Result<()> {
    enqueue(
        &pool(ctx)?,
        &event.data.product_id,
        &event.data.supplier_id,
        0,
    )
    .await?;
    Ok(())
}

#[evento::subscription]
async fn dequeue_on_sourcing_stopped<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<SourcingStopped>,
) -> anyhow::Result<()> {
    let db = pool(ctx)?;
    let Some(sourced) = Command::new(ctx.executor, db.clone())
        .load_sourced(&event.aggregate_id)
        .await?
    else {
        return Ok(());
    };
    dequeue(&db, &sourced.product_id).await?;
    Ok(())
}

/// A sale, or a cancellation, on something the shop does not hold itself.
///
/// Either way the level the supplier last reported is now further from the
/// truth than usual, and in opposite directions: a sale takes the shop
/// closer to selling what nobody has, while a cancellation puts units back
/// on sale that the supplier may no longer hold. Both are reasons to ask
/// again soon rather than in six hours.
async fn hurry_if_sourced<E: Executor>(
    ctx: &Context<'_, E>,
    stock_item_id: &str,
) -> anyhow::Result<()> {
    let db = pool(ctx)?;
    let Some(item) = timada_inventory::load_stock_availability(ctx.executor, stock_item_id).await?
    else {
        return Ok(());
    };
    if !item.location.is_warehouse() {
        return Ok(());
    }
    let due =
        timada_core::time::now_unix_secs()? as i64 + policy(ctx).every_reserved.as_secs() as i64;
    hurry(&db, &item.product_id, due).await?;
    Ok(())
}

#[evento::subscription]
async fn hurry_on_stock_reserved<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<StockReserved>,
) -> anyhow::Result<()> {
    hurry_if_sourced(ctx, &event.aggregate_id).await
}

#[evento::subscription]
async fn hurry_on_stock_released<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<StockReservationReleased>,
) -> anyhow::Result<()> {
    hurry_if_sourced(ctx, &event.aggregate_id).await
}
