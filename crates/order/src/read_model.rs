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
    aggregator::{
        OrderBuyerIdentified, OrderCancelled, OrderConfirmationResent, OrderDiscountApplied,
        OrderNumberAssigned, OrderPaid, OrderPlaced, OrderRatePinned, OrderReverseCharged,
        OrderSettled, OrderShipped, OrderTaxed,
    },
    query::load_order_details,
    value_object::{OrderStatus, Seller},
};

pub const ORDER_HISTORY_SUBSCRIPTION: &str = "order-history";

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct OrderHistoryRow {
    pub order_id: String,
    /// "C2026-000042"; `None` for an order placed without a number.
    pub order_number: Option<String>,
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
        .handler(status_on_order_settled())
        .handler(status_on_order_shipped())
        .handler(status_on_order_cancelled())
        // Both are committed together with `OrderPlaced`, whose handler
        // reads them through the details view.
        .skip::<OrderNumberAssigned>()
        .skip::<OrderTaxed>()
        .skip::<OrderBuyerIdentified>()
        .skip::<OrderReverseCharged>()
        .skip::<OrderRatePinned>()
        .skip::<OrderDiscountApplied>()
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
        "SELECT order_id, order_number, customer_id, placed_at, year, seller, status,
                total_minor, currency
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
        "SELECT order_id, order_number, customer_id, placed_at, year, seller, status,
                total_minor, currency
         FROM order_history
         WHERE customer_id = ?
         ORDER BY placed_at DESC, order_id",
    )
    .bind(customer_id)
    .fetch_all(db)
    .await
}

/// An order that is paid and waits for its parcel.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct OrderToShipRow {
    pub order_id: String,
    pub order_number: Option<String>,
    pub customer_id: String,
    pub placed_at: i64,
    /// When it was paid; its placing for an order paid before that was kept.
    pub waiting_since: i64,
    pub total_minor: i64,
    pub currency: String,
}

impl OrderToShipRow {
    pub fn total(&self) -> timada_core::Money {
        timada_core::Money::new(self.total_minor, &self.currency)
    }
}

/// The orders paid and not shipped yet, the one waiting longest first: what
/// an operator prepares next. The fulfillment saga never times these out —
/// somebody has to ship them. Orders held for a dispute on their payment
/// (`order_payment_hold`) are left out until the bank decides.
pub async fn orders_to_ship(
    db: &SqlitePool,
    limit: u32,
    offset: u32,
) -> sqlx::Result<Vec<OrderToShipRow>> {
    sqlx::query_as(
        "SELECT order_id, order_number, customer_id, placed_at,
                COALESCE(paid_at, placed_at) AS waiting_since, total_minor, currency
         FROM order_history
         WHERE status = 'paid'
           AND order_id NOT IN (SELECT order_id FROM order_payment_hold)
         ORDER BY waiting_since, order_id
         LIMIT ? OFFSET ?",
    )
    .bind(limit)
    .bind(offset)
    .fetch_all(db)
    .await
}

/// How many orders wait for their parcel, and how many of them have been
/// waiting since before `late_before` (Unix seconds).
pub async fn count_orders_to_ship(db: &SqlitePool, late_before: u64) -> sqlx::Result<(i64, i64)> {
    sqlx::query_as(
        "SELECT COUNT(*), COALESCE(SUM(COALESCE(paid_at, placed_at) < ?), 0)
         FROM order_history
         WHERE status = 'paid'
           AND order_id NOT IN (SELECT order_id FROM order_payment_hold)",
    )
    .bind(late_before as i64)
    .fetch_one(db)
    .await
}

/// Admin listing across all customers, newest first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListOrders {
    pub status: Option<OrderStatus>,
    /// Matches the start of the order number, or the whole order id.
    pub number: Option<String>,
    pub limit: u32,
    pub offset: u32,
}

impl Default for ListOrders {
    fn default() -> Self {
        Self {
            status: None,
            number: None,
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
        "SELECT order_id, order_number, customer_id, placed_at, year, seller, status,
                total_minor, currency
         FROM order_history
         WHERE (?1 IS NULL OR status = ?1)
           AND (?2 IS NULL OR order_number LIKE ?2 || '%' OR order_id = ?2)
         ORDER BY placed_at DESC, order_id
         LIMIT ?3 OFFSET ?4",
    )
    .bind(query.status.map(OrderStatus::as_str))
    .bind(number_filter(query.number.as_deref()))
    .bind(query.limit)
    .bind(query.offset)
    .fetch_all(db)
    .await
}

pub async fn count_orders(db: &SqlitePool, query: &ListOrders) -> sqlx::Result<i64> {
    sqlx::query_scalar(
        "SELECT COUNT(*) FROM order_history
         WHERE (?1 IS NULL OR status = ?1)
           AND (?2 IS NULL OR order_number LIKE ?2 || '%' OR order_id = ?2)",
    )
    .bind(query.status.map(OrderStatus::as_str))
    .bind(number_filter(query.number.as_deref()))
    .fetch_one(db)
    .await
}

fn number_filter(number: Option<&str>) -> Option<String> {
    let number = number?.trim().replace(['%', '_'], "");
    (!number.is_empty()).then_some(number)
}

/// The numbers of the given orders, for pages of other contexts that only
/// hold order ids. Orders without a number are absent.
pub async fn order_numbers_by_ids(
    db: &SqlitePool,
    order_ids: &[String],
) -> sqlx::Result<std::collections::HashMap<String, String>> {
    if order_ids.is_empty() {
        return Ok(Default::default());
    }
    let mut query = sqlx::QueryBuilder::<sqlx::Sqlite>::new(
        "SELECT order_id, order_number FROM order_history
         WHERE order_number IS NOT NULL AND order_id IN (",
    );
    let mut bound = query.separated(", ");
    for id in order_ids {
        bound.push_bind(id);
    }
    query.push(")");
    let rows: Vec<(String, String)> = query.build_query_as().fetch_all(db).await?;
    Ok(rows.into_iter().collect())
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

/// Paid, or settled with nothing to pay: from then on the order waits for its
/// parcel. The first date is kept, whatever is redelivered.
async fn set_paid<E: Executor>(
    ctx: &Context<'_, E>,
    order_id: &str,
    at: u64,
) -> anyhow::Result<()> {
    sqlx::query(
        "UPDATE order_history SET status = ?, paid_at = COALESCE(paid_at, ?) WHERE order_id = ?",
    )
    .bind(OrderStatus::Paid.as_str())
    .bind(at as i64)
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
    // The number and the total net of the discount come from companion
    // events committed together with this one: the details view is the one
    // place that folds them.
    let Some(order) = load_order_details(ctx.executor, &event.aggregate_id).await? else {
        anyhow::bail!("order {} placed but cannot be loaded", event.aggregate_id);
    };
    let seller = match &event.data.seller {
        Seller::Ldlc => "LDLC".to_owned(),
        Seller::Marketplace { name } => name.clone(),
    };
    sqlx::query(
        "INSERT OR IGNORE INTO order_history
            (order_id, order_number, customer_id, placed_at, year, seller, status, total_minor,
             currency)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&event.aggregate_id)
    .bind(&order.order_number)
    .bind(&event.data.customer_id)
    .bind(event.timestamp as i64)
    .bind(timada_core::time::year_of(event.timestamp))
    .bind(seller)
    .bind(OrderStatus::Placed.as_str())
    .bind(order.total.minor)
    .bind(&order.total.currency)
    .execute(&pool(ctx)?)
    .await?;
    Ok(())
}

#[evento::subscription]
async fn status_on_order_paid<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<OrderPaid>,
) -> anyhow::Result<()> {
    set_paid(ctx, &event.aggregate_id, event.timestamp).await
}

#[evento::subscription]
async fn status_on_order_settled<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<OrderSettled>,
) -> anyhow::Result<()> {
    set_paid(ctx, &event.aggregate_id, event.timestamp).await
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
