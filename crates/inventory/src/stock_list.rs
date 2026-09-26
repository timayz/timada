//! SQL list read model behind the admin's inventory section: one row per
//! stock item with its levels. Fed by the `inventory-stock-list` subscription;
//! the storefront keeps using [`crate::StockAvailabilityView`].

use evento::{
    Executor,
    metadata::Event,
    subscription::{Context, SubscriptionBuilder},
};
use sqlx::SqlitePool;

use crate::{
    aggregator::{
        StockItemRegistered, StockLevelSynced, StockReceived, StockReservationRejected,
        StockReservationReleased, StockReserved, StockReturned,
    },
    query::load_stock_availability,
};

/// Subscription key; the caller attaches the pool with `.data(pool)`.
pub const STOCK_LIST_SUBSCRIPTION: &str = "inventory-stock-list";

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct StockListRow {
    pub stock_item_id: String,
    pub product_id: String,
    /// [`crate::StockLocation::key`]: `warehouse` or `store:<id>`.
    pub location: String,
    pub on_hand: i64,
    pub reserved: i64,
    pub available: i64,
}

/// Filters for [`list_stock`]; `available_below` keeps the items running low.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListStock {
    pub available_below: Option<u32>,
    pub limit: u32,
    pub offset: u32,
}

impl Default for ListStock {
    fn default() -> Self {
        Self {
            available_below: None,
            limit: 50,
            offset: 0,
        }
    }
}

pub fn stock_list_subscription<E: Executor>() -> SubscriptionBuilder<E> {
    SubscriptionBuilder::new(STOCK_LIST_SUBSCRIPTION)
        .handler(refresh_on_stock_item_registered())
        .handler(refresh_on_stock_received())
        .handler(refresh_on_stock_returned())
        .handler(refresh_on_stock_level_synced())
        .handler(refresh_on_stock_reserved())
        .handler(refresh_on_stock_reservation_released())
        .skip::<StockReservationRejected>()
        .strict()
}

/// Stock items matching the filter, the emptiest first.
pub async fn list_stock(db: &SqlitePool, filter: &ListStock) -> sqlx::Result<Vec<StockListRow>> {
    sqlx::query_as(
        "SELECT stock_item_id, product_id, location, on_hand, reserved, available
         FROM inventory_stock_list
         WHERE (?1 IS NULL OR available < ?1)
         ORDER BY available, product_id, location
         LIMIT ?2 OFFSET ?3",
    )
    .bind(filter.available_below)
    .bind(filter.limit)
    .bind(filter.offset)
    .fetch_all(db)
    .await
}

pub async fn count_stock(db: &SqlitePool, available_below: Option<u32>) -> sqlx::Result<i64> {
    sqlx::query_scalar(
        "SELECT COUNT(*) FROM inventory_stock_list WHERE (?1 IS NULL OR available < ?1)",
    )
    .bind(available_below)
    .fetch_one(db)
    .await
}

/// Writes the item's current levels. Absolute values from the availability
/// projection rather than increments, so a redelivered event changes nothing.
async fn refresh<E: Executor>(ctx: &Context<'_, E>, stock_item_id: &str) -> anyhow::Result<()> {
    let db = ctx
        .get::<SqlitePool>()
        .ok_or_else(|| anyhow::anyhow!("SqlitePool missing from subscription context"))?;
    let Some(item) = load_stock_availability(ctx.executor, stock_item_id).await? else {
        anyhow::bail!("stock item {stock_item_id} cannot be loaded");
    };
    sqlx::query(
        "INSERT INTO inventory_stock_list
            (stock_item_id, product_id, location, on_hand, reserved, available)
         VALUES (?, ?, ?, ?, ?, ?)
         ON CONFLICT (stock_item_id) DO UPDATE SET
            on_hand = excluded.on_hand,
            reserved = excluded.reserved,
            available = excluded.available",
    )
    .bind(&item.id)
    .bind(&item.product_id)
    .bind(item.location.key())
    .bind(item.on_hand)
    .bind(item.reserved)
    .bind(item.available)
    .execute(&db)
    .await?;
    Ok(())
}

#[evento::subscription]
async fn refresh_on_stock_item_registered<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<StockItemRegistered>,
) -> anyhow::Result<()> {
    refresh(ctx, &event.aggregate_id).await
}

#[evento::subscription]
async fn refresh_on_stock_received<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<StockReceived>,
) -> anyhow::Result<()> {
    refresh(ctx, &event.aggregate_id).await
}

#[evento::subscription]
async fn refresh_on_stock_returned<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<StockReturned>,
) -> anyhow::Result<()> {
    refresh(ctx, &event.aggregate_id).await
}

#[evento::subscription]
async fn refresh_on_stock_level_synced<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<StockLevelSynced>,
) -> anyhow::Result<()> {
    refresh(ctx, &event.aggregate_id).await
}

#[evento::subscription]
async fn refresh_on_stock_reserved<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<StockReserved>,
) -> anyhow::Result<()> {
    refresh(ctx, &event.aggregate_id).await
}

#[evento::subscription]
async fn refresh_on_stock_reservation_released<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<StockReservationReleased>,
) -> anyhow::Result<()> {
    refresh(ctx, &event.aggregate_id).await
}
