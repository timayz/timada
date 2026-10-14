//! Queries over `orders_carts`: the carts shoppers are filling, so the
//! merchant sees what is left behind.

use sqlx::{FromRow, SqlitePool};

/// One row of `orders_carts`.
#[derive(Debug, Clone, FromRow, PartialEq, Eq)]
pub struct CartRow {
    pub tenant_id: String,
    pub id: String,
    /// The order it became, once checked out.
    pub order_id: Option<String>,
    pub line_count: i64,
    pub item_count: i64,
    /// Unix seconds.
    pub created_at: i64,
    /// Unix seconds of the shopper's last change.
    pub updated_at: i64,
}

/// The carts of a tenant holding something and not ordered yet, the most
/// recently touched first.
pub async fn open_carts(
    pool: &SqlitePool,
    tenant_id: &str,
    offset: i64,
    limit: i64,
) -> sqlx::Result<Vec<CartRow>> {
    sqlx::query_as(
        "SELECT tenant_id, id, order_id, line_count, item_count, created_at, updated_at
         FROM orders_carts
         WHERE tenant_id = ?1 AND order_id IS NULL AND item_count > 0
         ORDER BY updated_at DESC, id DESC LIMIT ?2 OFFSET ?3",
    )
    .bind(tenant_id)
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await
}

/// How many carts [`open_carts`] would list.
pub async fn count_open_carts(pool: &SqlitePool, tenant_id: &str) -> sqlx::Result<i64> {
    sqlx::query_scalar(
        "SELECT count(*) FROM orders_carts
         WHERE tenant_id = ?1 AND order_id IS NULL AND item_count > 0",
    )
    .bind(tenant_id)
    .fetch_one(pool)
    .await
}
