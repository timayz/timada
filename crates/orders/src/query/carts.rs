//! Queries over `orders_carts`: the carts shoppers are filling, so the
//! merchant sees what is left behind.

use timada_core::db::{Db, FromRow};

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
    db: &Db,
    tenant_id: &str,
    offset: i64,
    limit: i64,
) -> anyhow::Result<Vec<CartRow>> {
    db.fetch_all(
        "SELECT tenant_id, id, order_id, status, line_count, item_count, created_at, updated_at
         FROM orders_carts
         WHERE tenant_id = ?1 AND order_id IS NULL AND item_count > 0
         ORDER BY updated_at DESC, id DESC LIMIT ?2 OFFSET ?3",
        (tenant_id, limit, offset),
    )
    .await
}

/// How many carts [`open_carts`] would list.
pub async fn count_open_carts(db: &Db, tenant_id: &str) -> anyhow::Result<i64> {
    db.fetch_scalar(
        "SELECT count(*) FROM orders_carts
         WHERE tenant_id = ?1 AND order_id IS NULL AND item_count > 0",
        (tenant_id,),
    )
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
pub async fn paying_tenants_before(db: &Db, before: i64) -> anyhow::Result<Vec<String>> {
    let rows: Vec<(String,)> = db
        .fetch_all(
            "SELECT DISTINCT tenant_id FROM orders_carts
             WHERE status = 'paying' AND order_id IS NULL AND updated_at <= ?1
             ORDER BY tenant_id",
            (before,),
        )
        .await?;
    Ok(rows.into_iter().map(|(id,)| id).collect())
}

/// The carts of a tenant with a payment open, last touched at `before`
/// (unix seconds) or earlier: the checkouts whose holds may have lapsed. The
/// write model says which payments did; a cart touched since (an item added
/// while the card form was open) waits for a later look. A page of `limit`
/// rows, the ones after `after` when given, oldest first.
pub async fn paying_carts_before(
    db: &Db,
    tenant_id: &str,
    before: i64,
    after: Option<&CartCursor>,
    limit: i64,
) -> anyhow::Result<Vec<CartRow>> {
    let (after_updated_at, after_id) = after
        .map(|cursor| (cursor.updated_at, cursor.id.as_str()))
        .unwrap_or((i64::MIN, ""));
    db.fetch_all(
        "SELECT tenant_id, id, order_id, status, line_count, item_count, created_at, updated_at
         FROM orders_carts
         WHERE tenant_id = ?1 AND status = 'paying' AND order_id IS NULL AND updated_at <= ?2
           AND (updated_at > ?3 OR (updated_at = ?3 AND id > ?4))
         ORDER BY updated_at, id LIMIT ?5",
        (tenant_id, before, after_updated_at, after_id, limit),
    )
    .await
}
