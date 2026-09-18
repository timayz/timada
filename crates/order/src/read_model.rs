//! SQL list read model behind "Historique de vos commandes": one row per
//! order, filterable by customer and year. Fed by the `order-history`
//! subscription; the expanded order is served by [`crate::OrderDetailsView`].

use evento::{
    Executor,
    metadata::Event,
    subscription::{Context, SubscriptionBuilder},
};
use sqlx::SqlitePool;

use crate::{
    aggregator::{OrderCancelled, OrderConfirmationResent, OrderPaid, OrderPlaced, OrderShipped},
    value_object::{OrderStatus, Seller, order_total},
};

pub const ORDER_HISTORY_SUBSCRIPTION: &str = "order-history";

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct OrderHistoryRow {
    pub order_id: String,
    pub customer_id: String,
    pub placed_at: i64,
    pub year: i64,
    pub seller: String,
    pub status: String,
    pub total_minor: i64,
    pub currency: String,
}

pub fn order_history_subscription<E: Executor>() -> SubscriptionBuilder<E> {
    SubscriptionBuilder::new(ORDER_HISTORY_SUBSCRIPTION)
        .handler(insert_on_order_placed())
        .handler(status_on_order_paid())
        .handler(status_on_order_shipped())
        .handler(status_on_order_cancelled())
        .skip::<OrderConfirmationResent>()
        .strict()
}

/// Orders of a customer placed in `year`, newest first.
pub async fn history(
    db: &SqlitePool,
    customer_id: &str,
    year: i32,
) -> sqlx::Result<Vec<OrderHistoryRow>> {
    sqlx::query_as(
        "SELECT order_id, customer_id, placed_at, year, seller, status, total_minor, currency
         FROM order_history
         WHERE customer_id = ? AND year = ?
         ORDER BY placed_at DESC",
    )
    .bind(customer_id)
    .bind(year)
    .fetch_all(db)
    .await
}

/// Every order of a customer, all years, newest first.
pub async fn orders_of_customer(
    db: &SqlitePool,
    customer_id: &str,
) -> sqlx::Result<Vec<OrderHistoryRow>> {
    sqlx::query_as(
        "SELECT order_id, customer_id, placed_at, year, seller, status, total_minor, currency
         FROM order_history
         WHERE customer_id = ?
         ORDER BY placed_at DESC, order_id",
    )
    .bind(customer_id)
    .fetch_all(db)
    .await
}

/// Admin listing across all customers, newest first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListOrders {
    pub status: Option<OrderStatus>,
    pub limit: u32,
    pub offset: u32,
}

impl Default for ListOrders {
    fn default() -> Self {
        Self {
            status: None,
            limit: 50,
            offset: 0,
        }
    }
}

pub async fn list_orders(
    db: &SqlitePool,
    query: &ListOrders,
) -> sqlx::Result<Vec<OrderHistoryRow>> {
    sqlx::query_as(
        "SELECT order_id, customer_id, placed_at, year, seller, status, total_minor, currency
         FROM order_history
         WHERE (?1 IS NULL OR status = ?1)
         ORDER BY placed_at DESC, order_id
         LIMIT ?2 OFFSET ?3",
    )
    .bind(query.status.map(OrderStatus::as_str))
    .bind(query.limit)
    .bind(query.offset)
    .fetch_all(db)
    .await
}

pub async fn count_orders(db: &SqlitePool, status: Option<OrderStatus>) -> sqlx::Result<i64> {
    sqlx::query_scalar("SELECT COUNT(*) FROM order_history WHERE (?1 IS NULL OR status = ?1)")
        .bind(status.map(OrderStatus::as_str))
        .fetch_one(db)
        .await
}

fn pool<E: Executor>(ctx: &Context<'_, E>) -> anyhow::Result<SqlitePool> {
    ctx.get::<SqlitePool>()
        .ok_or_else(|| anyhow::anyhow!("SqlitePool missing from subscription context"))
}

async fn set_status<E: Executor>(
    ctx: &Context<'_, E>,
    order_id: &str,
    status: OrderStatus,
) -> anyhow::Result<()> {
    sqlx::query("UPDATE order_history SET status = ? WHERE order_id = ?")
        .bind(status.as_str())
        .bind(order_id)
        .execute(&pool(ctx)?)
        .await?;
    Ok(())
}

#[evento::subscription]
async fn insert_on_order_placed<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<OrderPlaced>,
) -> anyhow::Result<()> {
    let totals = order_total(
        &event.data.lines,
        &event.data.shipping_fee,
        &event.data.handling_fee,
    )?;
    let seller = match &event.data.seller {
        Seller::Ldlc => "LDLC".to_owned(),
        Seller::Marketplace { name } => name.clone(),
    };
    sqlx::query(
        "INSERT OR IGNORE INTO order_history
            (order_id, customer_id, placed_at, year, seller, status, total_minor, currency)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&event.aggregate_id)
    .bind(&event.data.customer_id)
    .bind(event.timestamp as i64)
    .bind(timada_core::time::year_of(event.timestamp))
    .bind(seller)
    .bind(OrderStatus::Placed.as_str())
    .bind(totals.total.minor)
    .bind(&totals.total.currency)
    .execute(&pool(ctx)?)
    .await?;
    Ok(())
}

#[evento::subscription]
async fn status_on_order_paid<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<OrderPaid>,
) -> anyhow::Result<()> {
    set_status(ctx, &event.aggregate_id, OrderStatus::Paid).await
}

#[evento::subscription]
async fn status_on_order_shipped<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<OrderShipped>,
) -> anyhow::Result<()> {
    set_status(ctx, &event.aggregate_id, OrderStatus::Shipped).await
}

#[evento::subscription]
async fn status_on_order_cancelled<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<OrderCancelled>,
) -> anyhow::Result<()> {
    set_status(ctx, &event.aggregate_id, OrderStatus::Cancelled).await
}
