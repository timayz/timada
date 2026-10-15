//! Queries over `orders_orders`, for the orders pages of a tenant.

use sqlx::{FromRow, SqlitePool};

use crate::command::OrderStatus;
use timada_catalog::format_money;

/// One row of `orders_orders`.
#[derive(Debug, Clone, FromRow, PartialEq, Eq)]
pub struct OrderRow {
    pub tenant_id: String,
    pub id: String,
    pub reference: String,
    pub cart_id: String,
    pub status: String,
    pub email: String,
    /// Who it ships to.
    pub name: String,
    pub country: String,
    pub item_count: i64,
    pub total_cents: i64,
    pub currency: String,
    /// Unix seconds.
    pub placed_at: i64,
    /// Unix seconds.
    pub updated_at: i64,
    /// The provider's id of the payment.
    pub payment_id: String,
    pub refunded_cents: i64,
    /// The discount it was paid with, if any.
    pub discount_id: Option<String>,
    pub discount_code: Option<String>,
    /// What the discount took off; `0` without one.
    pub discount_cents: i64,
}

impl OrderRow {
    pub fn status(&self) -> OrderStatus {
        OrderStatus::parse(&self.status).unwrap_or_default()
    }

    /// The total as a person reads it (`24.00 EUR`).
    pub fn total_label(&self) -> String {
        format_money(self.total_cents as u64, &self.currency)
    }
}

/// Which orders a page lists: one status, and a search over the reference,
/// the e-mail address and the name it ships to. Both are optional; the
/// default matches every order of the tenant.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct OrderFilter<'a> {
    pub status: Option<OrderStatus>,
    pub q: Option<&'a str>,
}

impl<'a> OrderFilter<'a> {
    pub fn status(mut self, status: Option<OrderStatus>) -> Self {
        self.status = status;
        self
    }

    pub fn search(mut self, q: Option<&'a str>) -> Self {
        self.q = q.map(str::trim).filter(|q| !q.is_empty());
        self
    }
}

/// A `LIKE` pattern matching `q` anywhere, with its wildcards escaped.
fn contains(q: &str) -> String {
    let escaped = q
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_");
    format!("%{escaped}%")
}

/// The orders of a tenant the filter matches, newest first.
pub async fn orders_of_tenant(
    pool: &SqlitePool,
    tenant_id: &str,
    filter: &OrderFilter<'_>,
    offset: i64,
    limit: i64,
) -> sqlx::Result<Vec<OrderRow>> {
    sqlx::query_as(
        "SELECT tenant_id, id, reference, cart_id, status, email, name, country, item_count,
            total_cents, currency, placed_at, updated_at, payment_id, refunded_cents,
            discount_id, discount_code, discount_cents
         FROM orders_orders
         WHERE tenant_id = ?1 AND (?2 IS NULL OR status = ?2)
           AND (?3 IS NULL OR reference LIKE ?3 ESCAPE '\\' OR email LIKE ?3 ESCAPE '\\'
                OR name LIKE ?3 ESCAPE '\\')
         ORDER BY placed_at DESC, id DESC LIMIT ?4 OFFSET ?5",
    )
    .bind(tenant_id)
    .bind(filter.status.map(OrderStatus::as_str))
    .bind(filter.q.map(contains))
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await
}

/// How many orders [`orders_of_tenant`] would list.
pub async fn count_orders(
    pool: &SqlitePool,
    tenant_id: &str,
    filter: &OrderFilter<'_>,
) -> sqlx::Result<i64> {
    sqlx::query_scalar(
        "SELECT count(*) FROM orders_orders
         WHERE tenant_id = ?1 AND (?2 IS NULL OR status = ?2)
           AND (?3 IS NULL OR reference LIKE ?3 ESCAPE '\\' OR email LIKE ?3 ESCAPE '\\'
                OR name LIKE ?3 ESCAPE '\\')",
    )
    .bind(tenant_id)
    .bind(filter.status.map(OrderStatus::as_str))
    .bind(filter.q.map(contains))
    .fetch_one(pool)
    .await
}

/// The latest orders of a tenant placed with a normalized address, newest
/// first: what a signed-in shopper sees as their history.
pub async fn orders_of_email(
    pool: &SqlitePool,
    tenant_id: &str,
    email: &str,
    limit: i64,
) -> sqlx::Result<Vec<OrderRow>> {
    sqlx::query_as(
        "SELECT tenant_id, id, reference, cart_id, status, email, name, country, item_count,
            total_cents, currency, placed_at, updated_at, payment_id, refunded_cents,
            discount_id, discount_code, discount_cents
         FROM orders_orders WHERE tenant_id = ?1 AND email = ?2
         ORDER BY placed_at DESC, id DESC LIMIT ?3",
    )
    .bind(tenant_id)
    .bind(email)
    .bind(limit)
    .fetch_all(pool)
    .await
}

/// An order that left, across every tenant this node follows.
#[derive(Debug, Clone, FromRow, PartialEq, Eq)]
pub struct FulfilledOrder {
    pub tenant_id: String,
    pub id: String,
    /// Unix seconds.
    pub fulfilled_at: i64,
}

/// The orders of every tenant fulfilled in `[from, to)` (Unix seconds),
/// grouped by tenant.
pub async fn fulfilled_between(
    pool: &SqlitePool,
    from: i64,
    to: i64,
) -> sqlx::Result<Vec<FulfilledOrder>> {
    sqlx::query_as(
        "SELECT tenant_id, id, fulfilled_at FROM orders_orders
         WHERE fulfilled_at >= ?1 AND fulfilled_at < ?2 AND status = 'fulfilled'
         ORDER BY tenant_id, fulfilled_at, id",
    )
    .bind(from)
    .bind(to)
    .fetch_all(pool)
    .await
}

/// One order of a tenant, by id.
pub async fn order_by_id(
    pool: &SqlitePool,
    tenant_id: &str,
    id: &str,
) -> sqlx::Result<Option<OrderRow>> {
    sqlx::query_as(
        "SELECT tenant_id, id, reference, cart_id, status, email, name, country, item_count,
            total_cents, currency, placed_at, updated_at, payment_id, refunded_cents,
            discount_id, discount_code, discount_cents
         FROM orders_orders WHERE tenant_id = ?1 AND id = ?2",
    )
    .bind(tenant_id)
    .bind(id)
    .fetch_optional(pool)
    .await
}

/// How many orders each discount of the tenant was paid with, by discount id
/// (cancelled ones too: the use was spent).
pub async fn discount_use_counts(
    pool: &SqlitePool,
    tenant_id: &str,
) -> sqlx::Result<std::collections::HashMap<String, i64>> {
    let rows: Vec<(String, i64)> = sqlx::query_as(
        "SELECT discount_id, count(*) FROM orders_orders
         WHERE tenant_id = ?1 AND discount_id IS NOT NULL GROUP BY discount_id",
    )
    .bind(tenant_id)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().collect())
}
