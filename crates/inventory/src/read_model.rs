//! Pending back-in-stock alerts per product, and the process that fires them
//! when warehouse stock goes from zero to some.

use evento::{
    Executor,
    metadata::Event,
    subscription::{Context, SubscriptionBuilder},
};
use sqlx::SqlitePool;

use crate::{
    aggregator::{
        BackInStockAlertCancelled, BackInStockAlertRequested, BackInStockAlertTriggered,
        StockLevelSynced, StockReceived, StockReturned,
    },
    command::{load_stock_item, trigger_alert},
};

/// Subscription key; the caller attaches the pool with `.data(pool)`.
pub const BACK_IN_STOCK_SUBSCRIPTION: &str = "inventory-back-in-stock";

/// Not strict: it listens to two aggregates and only cares about a subset of
/// each one's events.
pub fn back_in_stock_subscription<E: Executor>() -> SubscriptionBuilder<E> {
    SubscriptionBuilder::new(BACK_IN_STOCK_SUBSCRIPTION)
        .handler(insert_on_alert_requested())
        .handler(flag_on_alert_triggered())
        .handler(remove_on_alert_cancelled())
        .handler(trigger_on_stock_received())
        .handler(trigger_on_stock_returned())
        .handler(trigger_on_stock_level_synced())
}

/// Ids of alerts for `product_id` that have not fired yet.
pub async fn pending_alerts(db: &SqlitePool, product_id: &str) -> sqlx::Result<Vec<String>> {
    sqlx::query_scalar(
        "SELECT alert_id FROM inventory_back_in_stock_alert
         WHERE product_id = ? AND triggered = 0
         ORDER BY alert_id",
    )
    .bind(product_id)
    .fetch_all(db)
    .await
}

fn pool<E: Executor>(ctx: &Context<'_, E>) -> anyhow::Result<SqlitePool> {
    ctx.get::<SqlitePool>()
        .ok_or_else(|| anyhow::anyhow!("SqlitePool missing from subscription context"))
}

#[evento::subscription]
async fn insert_on_alert_requested<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<BackInStockAlertRequested>,
) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO inventory_back_in_stock_alert (alert_id, product_id)
         VALUES (?, ?)
         ON CONFLICT (alert_id) DO UPDATE SET triggered = 0",
    )
    .bind(&event.aggregate_id)
    .bind(&event.data.product_id)
    .execute(&pool(ctx)?)
    .await?;
    Ok(())
}

#[evento::subscription]
async fn flag_on_alert_triggered<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<BackInStockAlertTriggered>,
) -> anyhow::Result<()> {
    sqlx::query("UPDATE inventory_back_in_stock_alert SET triggered = 1 WHERE alert_id = ?")
        .bind(&event.aggregate_id)
        .execute(&pool(ctx)?)
        .await?;
    Ok(())
}

/// A cancelled alert is no longer pending; asking again inserts it anew.
#[evento::subscription]
async fn remove_on_alert_cancelled<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<BackInStockAlertCancelled>,
) -> anyhow::Result<()> {
    sqlx::query("DELETE FROM inventory_back_in_stock_alert WHERE alert_id = ?")
        .bind(&event.aggregate_id)
        .execute(&pool(ctx)?)
        .await?;
    Ok(())
}

/// Fires pending alerts when a warehouse item that was out of stock gets
/// `quantity` more units. The state is replayed up to now, so subtract them
/// to know what was available before.
async fn trigger_if_back<E: Executor>(
    ctx: &Context<'_, E>,
    stock_item_id: &str,
    quantity: u32,
) -> anyhow::Result<()> {
    let Some(item) = load_stock_item(ctx.executor, stock_item_id).await? else {
        anyhow::bail!("stock item {stock_item_id} missing for a stock increase");
    };
    if !item.location.is_warehouse() {
        return Ok(());
    }
    let available_before = item.available().saturating_sub(quantity);
    if available_before > 0 {
        return Ok(());
    }

    fire(ctx, &item.product_id).await
}

/// The same, for an absolute level: it carries no delta to subtract, so "was
/// it out of stock before?" cannot be asked of it. `sync_stock_level` only
/// writes when the level actually moved, and an alert that already fired is
/// no longer pending, so firing whenever the item has units is bounded and
/// never sends twice.
async fn trigger_if_available<E: Executor>(
    ctx: &Context<'_, E>,
    stock_item_id: &str,
) -> anyhow::Result<()> {
    let Some(item) = load_stock_item(ctx.executor, stock_item_id).await? else {
        anyhow::bail!("stock item {stock_item_id} missing for a level sync");
    };
    if !item.location.is_warehouse() || item.available() == 0 {
        return Ok(());
    }

    fire(ctx, &item.product_id).await
}

async fn fire<E: Executor>(ctx: &Context<'_, E>, product_id: &str) -> anyhow::Result<()> {
    for alert_id in pending_alerts(&pool(ctx)?, product_id).await? {
        trigger_alert(ctx.executor, alert_id).await?;
    }
    Ok(())
}

#[evento::subscription]
async fn trigger_on_stock_received<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<StockReceived>,
) -> anyhow::Result<()> {
    trigger_if_back(ctx, &event.aggregate_id, event.data.quantity).await
}

/// A customer's return can bring a product back too.
#[evento::subscription]
async fn trigger_on_stock_returned<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<StockReturned>,
) -> anyhow::Result<()> {
    trigger_if_back(ctx, &event.aggregate_id, event.data.quantity).await
}

/// A supplier restocking its own shelf brings the product back too.
#[evento::subscription]
async fn trigger_on_stock_level_synced<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<StockLevelSynced>,
) -> anyhow::Result<()> {
    trigger_if_available(ctx, &event.aggregate_id).await
}
