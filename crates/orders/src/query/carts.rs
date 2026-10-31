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
    /// `open`, or `paying` once a payment was asked for.
    pub status: String,
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
        "SELECT tenant_id, id, order_id, status, line_count, item_count, created_at, updated_at
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

/// Where a page of [`paying_carts_before`] stopped: the last row's
/// `updated_at` and `id`. The next page starts after it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CartCursor {
    pub updated_at: i64,
    pub id: String,
}

impl From<&CartRow> for CartCursor {
    fn from(row: &CartRow) -> Self {
        Self {
            updated_at: row.updated_at,
            id: row.id.clone(),
        }
    }
}

/// The tenants with a cart whose payment is open and that was last touched
/// at `before` (unix seconds) or earlier: the shops a sweep of lapsed
/// checkouts has anything to look at for. Each is then paged through
/// [`paying_carts_before`] on its own, so one shop's rows never hide
/// another's.
pub async fn paying_tenants_before(pool: &SqlitePool, before: i64) -> sqlx::Result<Vec<String>> {
    sqlx::query_scalar(
        "SELECT DISTINCT tenant_id FROM orders_carts
         WHERE status = 'paying' AND order_id IS NULL AND updated_at <= ?1
         ORDER BY tenant_id",
    )
    .bind(before)
    .fetch_all(pool)
    .await
}

/// The carts of a tenant with a payment open, last touched at `before`
/// (unix seconds) or earlier: the checkouts whose holds may have lapsed. The
/// write model says which payments did; a cart touched since (an item added
/// while the card form was open) waits for a later look. A page of `limit`
/// rows, the ones after `after` when given, oldest first.
pub async fn paying_carts_before(
    pool: &SqlitePool,
    tenant_id: &str,
    before: i64,
    after: Option<&CartCursor>,
    limit: i64,
) -> sqlx::Result<Vec<CartRow>> {
    let (after_updated_at, after_id) = after
        .map(|cursor| (cursor.updated_at, cursor.id.as_str()))
        .unwrap_or((i64::MIN, ""));
    sqlx::query_as(
        "SELECT tenant_id, id, order_id, status, line_count, item_count, created_at, updated_at
         FROM orders_carts
         WHERE tenant_id = ?1 AND status = 'paying' AND order_id IS NULL AND updated_at <= ?2
           AND (updated_at, id) > (?3, ?4)
         ORDER BY updated_at, id LIMIT ?5",
    )
    .bind(tenant_id)
    .bind(before)
    .bind(after_updated_at)
    .bind(after_id)
    .bind(limit)
    .fetch_all(pool)
    .await
}
