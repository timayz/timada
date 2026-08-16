//! The `admin_order_list` read model.
//!
//! One SQL table serving exactly one query shape: the admin orders list. It is
//! eventually consistent — a handler runs after the event is committed, so a
//! redirect straight after checkout may briefly not see the row. Nothing that
//! has to be correct *now* reads it: the customer's status page and the admin
//! detail page both replay [`load_order`](crate::view::load_order) instead.

use evento::metadata::Event;
use evento::subscription::{Context, Subscription, SubscriptionBuilder};
use sqlx::SqlitePool;
use timada_core::{Currency, Executor, Money};

use crate::aggregate::{
    OrderCancelled, OrderDelivered, OrderForwardedToSupplier, OrderPaid, OrderPlaced, OrderShipped,
};
use crate::saga::fulfillment_subscription;
use crate::state::OrderState;
use crate::view::OrderStatus;

/// Subscription key; also the cursor row's key in evento's `subscriber` table.
pub const ADMIN_SUBSCRIPTION: &str = "order-admin";

/// One row of the admin order list.
#[derive(Debug, sqlx::FromRow)]
pub struct AdminOrderRow {
    pub id: String,
    pub email: String,
    pub total_cents: i64,
    /// ISO code; parsed back into a [`Currency`] for display.
    pub currency: String,
    /// One of [`OrderStatus::as_str`].
    pub status: String,
    pub tracking_number: Option<String>,
    /// Epoch milliseconds — sub-second precision keeps the list ordered when
    /// several orders are placed within the same second.
    pub created_at: i64,
}

impl AdminOrderRow {
    pub fn total(&self) -> Money {
        Money::new(
            self.total_cents,
            Currency::from_code(&self.currency).unwrap_or_default(),
        )
    }

    /// `YYYY-MM-DD HH:MM UTC`, so the list is readable at a glance.
    pub fn created(&self) -> String {
        timada_core::format_utc_datetime(self.created_at)
    }
}

/// Newest orders first, capped so the page stays cheap.
pub async fn recent_orders(
    read_pool: &SqlitePool,
    limit: i64,
) -> anyhow::Result<Vec<AdminOrderRow>> {
    let rows = sqlx::query_as::<_, AdminOrderRow>(
        "SELECT id, email, total_cents, currency, status, tracking_number, created_at \
         FROM admin_order_list \
         ORDER BY created_at DESC, id DESC \
         LIMIT ?",
    )
    .bind(limit)
    .fetch_all(read_pool)
    .await?;

    Ok(rows)
}

/// The admin read-model subscription, unstarted.
///
/// Exposed so tests and the demo app's end-to-end run can drive it
/// deterministically with `.no_retry().run_once(&executor)` instead of racing a
/// background task.
pub fn admin_subscription(write_pool: SqlitePool) -> SubscriptionBuilder<Executor> {
    SubscriptionBuilder::<Executor>::new(ADMIN_SUBSCRIPTION)
        .data(write_pool)
        .handler(on_order_placed())
        .handler(on_order_paid())
        .handler(on_order_forwarded())
        .handler(on_order_shipped())
        .handler(on_order_delivered())
        .handler(on_order_cancelled())
        .strict()
}

/// Spawn every background subscription this crate owns: the admin read model
/// and the fulfillment saga.
///
/// The caller keeps the handles and calls `shutdown()` on them.
pub async fn start_subscriptions(state: &OrderState) -> anyhow::Result<Vec<Subscription>> {
    let saga = fulfillment_subscription(
        state.ctx.executor.clone(),
        state.registry.clone(),
        state.provider.clone(),
    )
    .start(&state.ctx.executor)
    .await?;

    let admin = admin_subscription(state.ctx.write_pool.clone())
        .start(&state.ctx.executor)
        .await?;

    tracing::info!("order subscriptions started");
    Ok(vec![saga, admin])
}

fn write_pool<E: evento::Executor>(ctx: &Context<'_, E>) -> anyhow::Result<SqlitePool> {
    ctx.get::<SqlitePool>().ok_or_else(|| {
        anyhow::anyhow!("`{ADMIN_SUBSCRIPTION}` subscription was started without a write pool")
    })
}

/// Every event after `OrderPlaced` moves the same row forward.
async fn set_status<E: evento::Executor>(
    ctx: &Context<'_, E>,
    order_id: &str,
    status: OrderStatus,
) -> anyhow::Result<()> {
    sqlx::query("UPDATE admin_order_list SET status = ? WHERE id = ?")
        .bind(status.as_str())
        .bind(order_id)
        .execute(&write_pool(ctx)?)
        .await?;

    Ok(())
}

#[evento::subscription]
async fn on_order_placed<E: evento::Executor>(
    ctx: &Context<'_, E>,
    event: Event<OrderPlaced>,
) -> anyhow::Result<()> {
    let created_at = i64::try_from(event.timestamp)
        .unwrap_or(i64::MAX / 1000)
        .saturating_mul(1000)
        .saturating_add(i64::from(event.timestamp_subsec));

    sqlx::query(
        "INSERT INTO admin_order_list \
             (id, email, total_cents, currency, status, created_at) \
         VALUES (?, ?, ?, ?, ?, ?) \
         ON CONFLICT(id) DO NOTHING",
    )
    .bind(&event.aggregate_id)
    .bind(&event.data.email)
    .bind(event.data.total.amount_cents)
    .bind(event.data.total.currency.code())
    .bind(OrderStatus::Placed.as_str())
    .bind(created_at)
    .execute(&write_pool(ctx)?)
    .await?;

    Ok(())
}

#[evento::subscription]
async fn on_order_paid<E: evento::Executor>(
    ctx: &Context<'_, E>,
    event: Event<OrderPaid>,
) -> anyhow::Result<()> {
    // The row is always there: one subscription replays an aggregate's events
    // in version order, so `OrderPlaced` was handled first.
    set_status(ctx, &event.aggregate_id, OrderStatus::Paid).await
}

#[evento::subscription]
async fn on_order_forwarded<E: evento::Executor>(
    ctx: &Context<'_, E>,
    event: Event<OrderForwardedToSupplier>,
) -> anyhow::Result<()> {
    set_status(ctx, &event.aggregate_id, OrderStatus::Forwarded).await
}

#[evento::subscription]
async fn on_order_shipped<E: evento::Executor>(
    ctx: &Context<'_, E>,
    event: Event<OrderShipped>,
) -> anyhow::Result<()> {
    sqlx::query("UPDATE admin_order_list SET status = ?, tracking_number = ? WHERE id = ?")
        .bind(OrderStatus::Shipped.as_str())
        .bind(&event.data.tracking_number)
        .bind(&event.aggregate_id)
        .execute(&write_pool(ctx)?)
        .await?;

    Ok(())
}

#[evento::subscription]
async fn on_order_delivered<E: evento::Executor>(
    ctx: &Context<'_, E>,
    event: Event<OrderDelivered>,
) -> anyhow::Result<()> {
    set_status(ctx, &event.aggregate_id, OrderStatus::Delivered).await
}

#[evento::subscription]
async fn on_order_cancelled<E: evento::Executor>(
    ctx: &Context<'_, E>,
    event: Event<OrderCancelled>,
) -> anyhow::Result<()> {
    set_status(ctx, &event.aggregate_id, OrderStatus::Cancelled).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_epoch_millis_as_a_readable_utc_stamp() {
        let row = |created_at| AdminOrderRow {
            id: "01".to_owned(),
            email: "a@b.c".to_owned(),
            total_cents: 0,
            currency: "EUR".to_owned(),
            status: "placed".to_owned(),
            tracking_number: None,
            created_at,
        };

        assert_eq!(row(0).created(), "1970-01-01 00:00 UTC");
        // 2024-02-29T13:45:07Z — a leap day, to prove the civil-date maths.
        assert_eq!(row(1_709_214_307_000).created(), "2024-02-29 13:45 UTC");
    }

    #[test]
    fn parses_the_stored_currency_back_for_display() {
        let row = AdminOrderRow {
            id: "01".to_owned(),
            email: "a@b.c".to_owned(),
            total_cents: 1234,
            currency: "USD".to_owned(),
            status: "placed".to_owned(),
            tracking_number: None,
            created_at: 0,
        };

        assert_eq!(row.total().to_string(), "12.34 USD");
    }
}
