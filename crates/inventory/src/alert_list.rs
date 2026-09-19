//! SQL list read model of back-in-stock alerts per customer: the shopper's
//! "Mes alertes" page. Fed by the `inventory-alert-list` subscription; the
//! process that fires alerts keeps its own table in [`crate::pending_alerts`].

use evento::{
    Executor,
    metadata::Event,
    subscription::{Context, SubscriptionBuilder},
};
use sqlx::SqlitePool;

use crate::aggregator::{BackInStockAlertRequested, BackInStockAlertTriggered};

/// Subscription key; the caller attaches the pool with `.data(pool)`.
pub const ALERT_LIST_SUBSCRIPTION: &str = "inventory-alert-list";

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct AlertListRow {
    pub alert_id: String,
    pub product_id: String,
    pub customer_id: String,
    /// Unix seconds of the latest request (an alert can be re-armed).
    pub requested_at: i64,
    /// When the product came back; `None` while the alert is pending.
    pub triggered_at: Option<i64>,
}

pub fn alert_list_subscription<E: Executor>() -> SubscriptionBuilder<E> {
    SubscriptionBuilder::new(ALERT_LIST_SUBSCRIPTION)
        .handler(upsert_on_alert_requested())
        .handler(date_on_alert_triggered())
        .strict()
}

/// A customer's alerts: the ones that fired first, then the newest requests.
pub async fn alerts_of_customer(
    db: &SqlitePool,
    customer_id: &str,
) -> sqlx::Result<Vec<AlertListRow>> {
    sqlx::query_as(
        "SELECT alert_id, product_id, customer_id, requested_at, triggered_at
         FROM inventory_alert_list
         WHERE customer_id = ?
         ORDER BY triggered_at IS NULL, requested_at DESC, alert_id",
    )
    .bind(customer_id)
    .fetch_all(db)
    .await
}

fn pool<E: Executor>(ctx: &Context<'_, E>) -> anyhow::Result<SqlitePool> {
    ctx.get::<SqlitePool>()
        .ok_or_else(|| anyhow::anyhow!("SqlitePool missing from subscription context"))
}

/// A request on an alert that already fired arms it again.
#[evento::subscription]
async fn upsert_on_alert_requested<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<BackInStockAlertRequested>,
) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO inventory_alert_list (alert_id, product_id, customer_id, requested_at)
         VALUES (?, ?, ?, ?)
         ON CONFLICT (alert_id) DO UPDATE SET
            requested_at = excluded.requested_at,
            triggered_at = NULL",
    )
    .bind(&event.aggregate_id)
    .bind(&event.data.product_id)
    .bind(&event.data.customer_id)
    .bind(event.timestamp as i64)
    .execute(&pool(ctx)?)
    .await?;
    Ok(())
}

#[evento::subscription]
async fn date_on_alert_triggered<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<BackInStockAlertTriggered>,
) -> anyhow::Result<()> {
    sqlx::query("UPDATE inventory_alert_list SET triggered_at = ? WHERE alert_id = ?")
        .bind(event.timestamp as i64)
        .bind(&event.aggregate_id)
        .execute(&pool(ctx)?)
        .await?;
    Ok(())
}
