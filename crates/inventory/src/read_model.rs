//! Pending back-in-stock alerts per product, and the process that fires them
//! when warehouse stock goes from zero to some.

use evento::{
    Executor,
    metadata::Event,
    subscription::{Context, SubscriptionBuilder},
};
use sqlx::SqlitePool;

use crate::{
    aggregator::{BackInStockAlertRequested, BackInStockAlertTriggered, StockReceived},
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
        .handler(trigger_on_stock_received())
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
        "INSERT OR IGNORE INTO inventory_back_in_stock_alert (alert_id, product_id)
         VALUES (?, ?)",
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

/// Fires pending alerts when a warehouse item that was out of stock receives
/// units. The state is replayed up to now, so subtract this receipt to know
/// what was available before it.
#[evento::subscription]
async fn trigger_on_stock_received<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<StockReceived>,
) -> anyhow::Result<()> {
    let Some(item) = load_stock_item(ctx.executor, &event.aggregate_id).await? else {
        anyhow::bail!(
            "stock item {} missing for StockReceived",
            event.aggregate_id
        );
    };
    if !item.location.is_warehouse() {
        return Ok(());
    }
    let available_before = item.available().saturating_sub(event.data.quantity);
    if available_before > 0 {
        return Ok(());
    }

    for alert_id in pending_alerts(&pool(ctx)?, &item.product_id).await? {
        trigger_alert(ctx.executor, alert_id).await?;
    }
    Ok(())
}
