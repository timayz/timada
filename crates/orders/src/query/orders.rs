//! Queries over `orders_orders`, for the orders pages of a tenant.

use timada_core::db::{Db, FromRow};

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
    /// Who supplies some of the goods once placed (`aliexpress`).
    pub supplier: Option<String>,
    /// `held`, `capturing`, `capture_failed`, `paid` or `released`
    /// ([`OrderState::payment_state`](crate::OrderState::payment_state)).
    pub payment_state: String,
    /// Unix seconds the hold on the shopper's card lapses at, when known.
    pub capture_before: Option<i64>,
    /// Unix seconds the money was taken at.
    pub paid_at: Option<i64>,
}

impl OrderRow {
    pub fn status(&self) -> OrderStatus {
        OrderStatus::parse(&self.status).unwrap_or_default()
    }

    /// The total as a person reads it (`24.00 €`).
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

use timada_core::like_contains as contains;

/// The orders of a tenant the filter matches, newest first.
pub async fn orders_of_tenant(
    db: &Db,
    tenant_id: &str,
    filter: &OrderFilter<'_>,
    offset: i64,
    limit: i64,
) -> anyhow::Result<Vec<OrderRow>> {
    db.fetch_all(
        "SELECT tenant_id, id, reference, cart_id, status, email, name, country, item_count,
            total_cents, currency, placed_at, updated_at, payment_id, refunded_cents,
            discount_id, discount_code, discount_cents, supplier, payment_state, capture_before,
            paid_at
         FROM orders_orders
         WHERE tenant_id = ?1 AND (?2 IS NULL OR status = ?2)
           AND (?3 IS NULL OR reference LIKE ?3 ESCAPE '\\' OR email LIKE ?3 ESCAPE '\\'
                OR name LIKE ?3 ESCAPE '\\')
         ORDER BY placed_at DESC, id DESC LIMIT ?4 OFFSET ?5",
        (
            tenant_id,
            filter.status.map(OrderStatus::as_str),
            filter.q.map(contains),
            limit,
            offset,
        ),
    )
    .await
}

/// How many orders [`orders_of_tenant`] would list.
pub async fn count_orders(
    db: &Db,
    tenant_id: &str,
    filter: &OrderFilter<'_>,
) -> anyhow::Result<i64> {
    db.fetch_scalar(
        "SELECT count(*) FROM orders_orders
         WHERE tenant_id = ?1 AND (?2 IS NULL OR status = ?2)
           AND (?3 IS NULL OR reference LIKE ?3 ESCAPE '\\' OR email LIKE ?3 ESCAPE '\\'
                OR name LIKE ?3 ESCAPE '\\')",
        (
            tenant_id,
            filter.status.map(OrderStatus::as_str),
            filter.q.map(contains),
        ),
    )
    .await
}

/// The latest orders of a tenant placed with a normalized address, newest
/// first: what a signed-in shopper sees as their history.
pub async fn orders_of_email(
    db: &Db,
    tenant_id: &str,
    email: &str,
    limit: i64,
) -> anyhow::Result<Vec<OrderRow>> {
    db.fetch_all(
        "SELECT tenant_id, id, reference, cart_id, status, email, name, country, item_count,
            total_cents, currency, placed_at, updated_at, payment_id, refunded_cents,
            discount_id, discount_code, discount_cents, supplier, payment_state, capture_before,
            paid_at
         FROM orders_orders WHERE tenant_id = ?1 AND email = ?2
         ORDER BY placed_at DESC, id DESC LIMIT ?3",
        (tenant_id, email, limit),
    )
    .await
}

/// An order whose money needs looking after, across every tenant.
#[derive(Debug, Clone, FromRow, PartialEq, Eq)]
pub struct PaymentDue {
    pub tenant_id: String,
    pub id: String,
}

/// The orders of every tenant whose money needs looking after at `now`
/// (Unix seconds): a capture or a cancellation asked before `asked_before`
/// and still unanswered, a hold whose time ran out, or an order of goods the
/// shop holds itself, placed before `asked_before` and still only held.
/// Grouped by tenant, oldest first. An order whose hold went back and that
/// stays open (its supplier may have it) waits for the merchant: nothing is
/// left to settle with the provider until they ask to cancel it.
pub async fn payments_due(
    db: &Db,
    now: i64,
    asked_before: i64,
    limit: i64,
) -> anyhow::Result<Vec<PaymentDue>> {
    db.fetch_all(
        "SELECT tenant_id, id FROM orders_orders
         WHERE (status = 'authorized' OR cancel_requested_at IS NOT NULL)
           AND (payment_state <> 'released' OR cancel_requested_at IS NOT NULL)
           AND (capture_requested_at < ?2 OR cancel_requested_at < ?2 OR capture_before < ?1
                OR (supplier IS NULL AND payment_state = 'held' AND placed_at < ?2))
         ORDER BY tenant_id, placed_at, id LIMIT ?3",
        (now, asked_before, limit),
    )
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
pub async fn fulfilled_between(db: &Db, from: i64, to: i64) -> anyhow::Result<Vec<FulfilledOrder>> {
    db.fetch_all(
        "SELECT tenant_id, id, fulfilled_at FROM orders_orders
         WHERE fulfilled_at >= ?1 AND fulfilled_at < ?2 AND status = 'fulfilled'
         ORDER BY tenant_id, fulfilled_at, id",
        (from, to),
    )
    .await
}

/// Whether any order of the tenant ever held the product, cancelled or
/// refunded ones included: once sold, a product stays on record and may be
/// archived but not deleted.
pub async fn product_ever_ordered(
    db: &Db,
    tenant_id: &str,
    product_id: &str,
) -> anyhow::Result<bool> {
    let count: i64 = db
        .fetch_scalar(
            "SELECT count(*) FROM orders_order_products WHERE tenant_id = ?1 AND product_id = ?2",
            (tenant_id, product_id),
        )
        .await?;
    Ok(count > 0)
}

/// One order of a tenant, by id.
pub async fn order_by_id(db: &Db, tenant_id: &str, id: &str) -> anyhow::Result<Option<OrderRow>> {
    db.fetch_optional(
        "SELECT tenant_id, id, reference, cart_id, status, email, name, country, item_count,
            total_cents, currency, placed_at, updated_at, payment_id, refunded_cents,
            discount_id, discount_code, discount_cents, supplier, payment_state, capture_before,
            paid_at
         FROM orders_orders WHERE tenant_id = ?1 AND id = ?2",
        (tenant_id, id),
    )
    .await
}

/// How many orders each discount of the tenant was used by, by discount id:
/// cancelled ones too once paid (the use was spent), not one cancelled
/// before it was, whose use went back.
pub async fn discount_use_counts(
    db: &Db,
    tenant_id: &str,
) -> anyhow::Result<std::collections::HashMap<String, i64>> {
    let rows: Vec<(String, i64)> = db
        .fetch_all(
            "SELECT discount_id, count(*) FROM orders_orders
         WHERE tenant_id = ?1 AND discount_id IS NOT NULL AND payment_state <> 'released'
         GROUP BY discount_id",
            (tenant_id,),
        )
        .await?;
    Ok(rows.into_iter().collect())
}
