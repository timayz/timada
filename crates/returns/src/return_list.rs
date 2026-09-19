//! SQL list read model of returns: the admin's queue and the returns shown
//! on an order's page. Fed by the `return-list` subscription; one return in
//! full is served by [`crate::ReturnView`].

use evento::{
    Executor,
    metadata::Event,
    subscription::{Context, SubscriptionBuilder},
};
use sqlx::SqlitePool;

use crate::{
    aggregator::{
        ReturnApproved, ReturnCancelled, ReturnCompleted, ReturnReceived, ReturnRefused,
        ReturnRequested,
    },
    query::load_return,
    value_object::ReturnStatus,
};

/// Subscription key; the caller attaches the pool with `.data(pool)`.
pub const RETURN_LIST_SUBSCRIPTION: &str = "return-list";

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct ReturnListRow {
    pub return_id: String,
    pub rma_number: String,
    pub order_id: String,
    pub customer_id: String,
    /// [`ReturnStatus::as_str`].
    pub status: String,
    pub reason: String,
    /// Units asked to be returned.
    pub units: i64,
    pub refunded_minor: i64,
    pub credited_minor: i64,
    pub currency: String,
    pub requested_at: i64,
}

/// Filters for [`list_returns`], the admin queue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListReturns {
    pub status: Option<ReturnStatus>,
    pub limit: u32,
    pub offset: u32,
}

impl Default for ListReturns {
    fn default() -> Self {
        Self {
            status: None,
            limit: 50,
            offset: 0,
        }
    }
}

pub fn return_list_subscription<E: Executor>() -> SubscriptionBuilder<E> {
    SubscriptionBuilder::new(RETURN_LIST_SUBSCRIPTION)
        .handler(refresh_on_return_requested())
        .handler(refresh_on_return_approved())
        .handler(refresh_on_return_refused())
        .handler(refresh_on_return_cancelled())
        .handler(refresh_on_return_received())
        .handler(refresh_on_return_completed())
        .strict()
}

/// Returns across all orders, oldest first: the queue is worked from the top.
pub async fn list_returns(
    db: &SqlitePool,
    filter: &ListReturns,
) -> sqlx::Result<Vec<ReturnListRow>> {
    sqlx::query_as(
        "SELECT return_id, rma_number, order_id, customer_id, status, reason, units,
                refunded_minor, credited_minor, currency, requested_at
         FROM return_list
         WHERE (?1 IS NULL OR status = ?1)
         ORDER BY requested_at, return_id
         LIMIT ?2 OFFSET ?3",
    )
    .bind(filter.status.map(ReturnStatus::as_str))
    .bind(filter.limit)
    .bind(filter.offset)
    .fetch_all(db)
    .await
}

pub async fn count_returns(db: &SqlitePool, status: Option<ReturnStatus>) -> sqlx::Result<i64> {
    sqlx::query_scalar("SELECT COUNT(*) FROM return_list WHERE (?1 IS NULL OR status = ?1)")
        .bind(status.map(ReturnStatus::as_str))
        .fetch_one(db)
        .await
}

/// The returns of one order, newest first.
pub async fn returns_of_order(db: &SqlitePool, order_id: &str) -> sqlx::Result<Vec<ReturnListRow>> {
    sqlx::query_as(
        "SELECT return_id, rma_number, order_id, customer_id, status, reason, units,
                refunded_minor, credited_minor, currency, requested_at
         FROM return_list
         WHERE order_id = ?
         ORDER BY requested_at DESC, return_id DESC",
    )
    .bind(order_id)
    .fetch_all(db)
    .await
}

/// Units of each product of an order that its returns already hold: what a
/// return form must subtract from the quantities bought. Straight from the
/// write-side claims, so it is exact.
pub async fn claimed_quantities(
    db: &SqlitePool,
    order_id: &str,
) -> sqlx::Result<Vec<(String, i64)>> {
    sqlx::query_as(
        "SELECT product_id, SUM(quantity) FROM return_claim
         WHERE order_id = ?
         GROUP BY product_id",
    )
    .bind(order_id)
    .fetch_all(db)
    .await
}

/// Writes the return as it stands now: absolute values from the view, so a
/// redelivery changes nothing.
async fn refresh<E: Executor>(ctx: &Context<'_, E>, return_id: &str) -> anyhow::Result<()> {
    let db = ctx
        .get::<SqlitePool>()
        .ok_or_else(|| anyhow::anyhow!("SqlitePool missing from subscription context"))?;
    let Some(view) = load_return(ctx.executor, return_id).await? else {
        anyhow::bail!("return {return_id} cannot be loaded");
    };
    sqlx::query(
        "INSERT INTO return_list
            (return_id, rma_number, order_id, customer_id, status, reason, units,
             refunded_minor, credited_minor, currency, requested_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT (return_id) DO UPDATE SET
            status = excluded.status,
            refunded_minor = excluded.refunded_minor,
            credited_minor = excluded.credited_minor",
    )
    .bind(&view.id)
    .bind(&view.rma_number)
    .bind(&view.order_id)
    .bind(&view.customer_id)
    .bind(view.status.as_str())
    .bind(&view.reason)
    .bind(view.units())
    .bind(view.money.minor)
    .bind(view.credit.minor)
    .bind(&view.money.currency)
    .bind(view.requested_at as i64)
    .execute(&db)
    .await?;
    Ok(())
}

#[evento::subscription]
async fn refresh_on_return_requested<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<ReturnRequested>,
) -> anyhow::Result<()> {
    refresh(ctx, &event.aggregate_id).await
}

#[evento::subscription]
async fn refresh_on_return_approved<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<ReturnApproved>,
) -> anyhow::Result<()> {
    refresh(ctx, &event.aggregate_id).await
}

#[evento::subscription]
async fn refresh_on_return_refused<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<ReturnRefused>,
) -> anyhow::Result<()> {
    refresh(ctx, &event.aggregate_id).await
}

#[evento::subscription]
async fn refresh_on_return_cancelled<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<ReturnCancelled>,
) -> anyhow::Result<()> {
    refresh(ctx, &event.aggregate_id).await
}

#[evento::subscription]
async fn refresh_on_return_received<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<ReturnReceived>,
) -> anyhow::Result<()> {
    refresh(ctx, &event.aggregate_id).await
}

#[evento::subscription]
async fn refresh_on_return_completed<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<ReturnCompleted>,
) -> anyhow::Result<()> {
    refresh(ctx, &event.aggregate_id).await
}
