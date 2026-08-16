//! Read model backing the `self_inventory` provider's stock queries:
//! one row per product that ever had a stock adjustment.

use anyhow::Result;
use evento::Executor;
use evento::metadata::Event;
use evento::sql::RwSqlite;
use evento::subscription::{Context, Subscription, SubscriptionBuilder};
use sqlx::SqlitePool;
use sqlx_migrator::{sqlite_migration, vec_box};

use crate::inventory::StockAdjusted;

pub struct M0004CreateStockLevels;

sqlite_migration!(
    M0004CreateStockLevels,
    "timada",
    "m0004_create_stock_levels",
    vec_box![],
    vec_box![(
        "CREATE TABLE stock_levels (
            product_id TEXT PRIMARY KEY,
            available INTEGER NOT NULL DEFAULT 0
        )",
        "DROP TABLE stock_levels"
    )]
);

/// Available stock for one product; zero when never adjusted.
pub async fn available(db: &SqlitePool, product_id: &str) -> Result<i64> {
    let row: Option<(i64,)> =
        sqlx::query_as("SELECT available FROM stock_levels WHERE product_id = ?")
            .bind(product_id)
            .fetch_optional(db)
            .await?;
    Ok(row.map_or(0, |(available,)| available))
}

fn db<E: Executor>(ctx: &Context<'_, E>) -> Result<SqlitePool> {
    ctx.get::<SqlitePool>()
        .ok_or_else(|| anyhow::anyhow!("SqlitePool not injected into subscription"))
}

#[evento::subscription]
async fn on_stock_adjusted<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<StockAdjusted>,
) -> Result<()> {
    // Absolute value from the event, so replays are idempotent.
    sqlx::query(
        "INSERT INTO stock_levels (product_id, available) VALUES (?, ?)
         ON CONFLICT(product_id) DO UPDATE SET available = excluded.available",
    )
    .bind(&event.aggregate_id)
    .bind(event.data.available)
    .execute(&db(ctx)?)
    .await?;
    Ok(())
}

/// Start the subscription that keeps `stock_levels` up to date.
pub(crate) async fn start(executor: &RwSqlite, write_pool: SqlitePool) -> Result<Subscription> {
    SubscriptionBuilder::<RwSqlite>::new("stock-levels")
        .handler(on_stock_adjusted())
        .data(write_pool)
        .all()
        .start(executor)
        .await
}
