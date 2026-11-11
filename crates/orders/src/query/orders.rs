//! Queries over `orders_orders`, for the orders pages of a tenant.

use timada_core::MailStatus;
use timada_core::db::{Db, FromRow, Tally};

use crate::command::{OrderRefund, OrderState, OrderStatus, OutsideSettlement};
use timada_catalog::format_money;

/// An order as its row keeps it: the whole [`OrderState`], with the facts
/// that are the shop's alone and left out of what the store API answers a
/// storefront (`#[serde(skip)]` on the state) carried beside it, so the
/// back-office page reads the order as the write model has it.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct OrderRecord {
    #[serde(flatten)]
    pub order: OrderState,
    pub settled_outside: Option<OutsideSettlement>,
    pub hold_reminded_at: Option<u64>,
    pub supplier_uncertain: bool,
    pub supplier_uncertain_at: Option<u64>,
    pub supplier_attempt_open: bool,
    pub fulfilled_by: Option<String>,
    #[serde(default)]
    pub invoicing_failure: Option<String>,
    #[serde(default)]
    pub refunds: Vec<OrderRefund>,
}

impl OrderRecord {
    pub fn of(order: &OrderState) -> Self {
        Self {
            order: order.clone(),
            settled_outside: order.settled_outside.clone(),
            hold_reminded_at: order.hold_reminded_at,
            supplier_uncertain: order.supplier_uncertain,
            supplier_uncertain_at: order.supplier_uncertain_at,
            supplier_attempt_open: order.supplier_attempt_open,
            fulfilled_by: order.fulfilled_by.clone(),
            invoicing_failure: order.invoicing_failure.clone(),
            refunds: order.refunds.clone(),
        }
    }

    pub fn to_json(&self) -> anyhow::Result<String> {
        Ok(serde_json::to_string(self)?)
    }

    /// The order whole again.
    pub fn into_state(self) -> OrderState {
        OrderState {
            settled_outside: self.settled_outside,
            hold_reminded_at: self.hold_reminded_at,
            supplier_uncertain: self.supplier_uncertain,
            supplier_uncertain_at: self.supplier_uncertain_at,
            supplier_attempt_open: self.supplier_attempt_open,
            fulfilled_by: self.fulfilled_by,
            invoicing_failure: self.invoicing_failure,
            refunds: self.refunds,
            ..self.order
        }
    }
}

/// An order as the read model has it, whole, or `None` for one the tenant
/// has not. A moment behind its stream: a page that follows an action on
/// the order shows the action once the row caught up.
pub async fn order_state(db: &Db, tenant_id: &str, id: &str) -> anyhow::Result<Option<OrderState>> {
    let state: Option<(String,)> = db
        .fetch_optional(
            "SELECT state FROM orders_orders WHERE tenant_id = ?1 AND id = ?2",
            (tenant_id, id),
        )
        .await?;
    state
        .map(|(state,)| {
            serde_json::from_str::<OrderRecord>(&state)
                .map(OrderRecord::into_state)
                .map_err(anyhow::Error::from)
        })
        .transpose()
}

/// One row of `orders_order_mails`: where a mail to the shopper about an
/// order stands, once a node took its sending.
#[derive(Debug, Clone, FromRow, PartialEq, Eq)]
pub struct OrderMailRow {
    /// The mail's kind key (`order_confirmation`).
    pub mail: String,
    /// Where it stands ([`MailStatus::as_str`]).
    pub status: String,
    /// How many sends were tried.
    pub attempts: i64,
    /// What the last failure said, or `""`.
    pub reason: String,
    /// Unix seconds: when it was handed to the mail server.
    pub sent_at: Option<i64>,
    /// Unix seconds.
    pub updated_at: i64,
}

impl OrderMailRow {
    pub fn status(&self) -> MailStatus {
        MailStatus::parse(&self.status)
    }
}

/// The mails to the shopper about an order, by kind.
pub async fn order_mails(
    db: &Db,
    tenant_id: &str,
    order_id: &str,
) -> anyhow::Result<Vec<OrderMailRow>> {
    db.fetch_all(
        "SELECT mail, status, attempts, reason, sent_at, updated_at FROM orders_order_mails
         WHERE tenant_id = ?1 AND order_id = ?2 ORDER BY updated_at, mail",
        (tenant_id, order_id),
    )
    .await
}

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
    /// `held`, `capturing`, `capture_failed`, `paid`, `settled_outside` or
    /// `released` ([`OrderState::payment_state`](crate::OrderState::payment_state)).
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

/// How many orders a tenant has, whatever their status: what its home
/// shows. Kept by the read model as it writes the rows.
pub const ORDERS: Tally = Tally {
    table: "orders_tallies",
    name: "orders",
    counted: "SELECT count(*) FROM orders_orders WHERE tenant_id = ?1 AND id = ?2",
};

/// How many orders a tenant has, read from [`ORDERS`].
pub async fn order_count(db: &Db, tenant_id: &str) -> anyhow::Result<i64> {
    ORDERS.read(db, tenant_id).await
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
/// (Unix seconds): a capture, a cancellation or a refund asked before
/// `asked_before` and still unanswered, a hold whose time ran out, or an
/// order of goods the shop holds itself, placed before `asked_before` and
/// still only held. Grouped by tenant, oldest first. An order whose hold
/// went back and that stays open (its supplier may have it) waits for the
/// merchant: nothing is left to settle with the provider until they ask to
/// cancel it, or a refund is asked.
pub async fn payments_due(
    db: &Db,
    now: i64,
    asked_before: i64,
    limit: i64,
) -> anyhow::Result<Vec<PaymentDue>> {
    db.fetch_all(
        "SELECT tenant_id, id FROM orders_orders
         WHERE (status = 'authorized' OR cancel_requested_at IS NOT NULL
                OR refund_requested_at IS NOT NULL)
           AND (payment_state <> 'released' OR cancel_requested_at IS NOT NULL
                OR refund_requested_at IS NOT NULL)
           AND (capture_requested_at < ?2 OR cancel_requested_at < ?2
                OR refund_requested_at < ?2 OR capture_before < ?1
                OR (supplier IS NULL AND payment_state = 'held' AND placed_at < ?2))
         ORDER BY tenant_id, placed_at, id LIMIT ?3",
        (now, asked_before, limit),
    )
    .await
}

/// An order whose merchant is to be reminded that its hold lapses soon.
#[derive(Debug, Clone, FromRow, PartialEq, Eq)]
pub struct HoldReminderDue {
    pub tenant_id: String,
    pub id: String,
}

/// The orders of every tenant whose merchant is due a reminder at `now`
/// (unix seconds): their money still only held while the supplier may have
/// them, the reminder not sent, and its time
/// ([`hold_reminder_due_at`](crate::OrderState::hold_reminder_due_at))
/// come. Grouped by tenant, the soonest to lapse first; the write model
/// decides.
pub async fn hold_reminders_due(
    db: &Db,
    now: i64,
    limit: i64,
) -> anyhow::Result<Vec<HoldReminderDue>> {
    db.fetch_all(
        "SELECT tenant_id, id FROM orders_orders
         WHERE hold_reminder_at IS NOT NULL AND hold_reminder_at <= ?1
         ORDER BY tenant_id, hold_reminder_at, id LIMIT ?2",
        (now, limit),
    )
    .await
}

/// How many payments the tenant still holds or owes, each needing its
/// provider account to settle:
/// - orders whose money is held on the shopper's card (an authorized order
///   whose hold went back holds nothing), or asked to be taken, cancelled or
///   refunded;
/// - carts with a payment open (a card form that may still confirm), one on
///   its way back, or a request to pay being answered.
pub async fn money_owed(db: &Db, tenant_id: &str) -> anyhow::Result<i64> {
    db.fetch_scalar(
        "SELECT (
            SELECT count(*) FROM orders_orders WHERE tenant_id = ?1
              AND ((status = 'authorized' AND payment_state <> 'released')
                   OR capture_requested_at IS NOT NULL OR cancel_requested_at IS NOT NULL
                   OR refund_requested_at IS NOT NULL)
         ) + (
            SELECT count(*) FROM orders_carts WHERE tenant_id = ?1
              AND (status = 'paying' OR return_requested_at IS NOT NULL
                   OR unanswered_since IS NOT NULL)
         )",
        (tenant_id,),
    )
    .await
}

/// The provider's id of one payment the tenant's orders still hold or owe,
/// if any: what a key is asked to read to show it opens the account those
/// payments were made through.
pub async fn owed_payment_id(db: &Db, tenant_id: &str) -> anyhow::Result<Option<String>> {
    let row: Option<(String,)> = db
        .fetch_optional(
            "SELECT payment_id FROM orders_orders WHERE tenant_id = ?1
               AND ((status = 'authorized' AND payment_state <> 'released')
                    OR capture_requested_at IS NOT NULL OR cancel_requested_at IS NOT NULL
                    OR refund_requested_at IS NOT NULL)
             ORDER BY placed_at, id LIMIT 1",
            (tenant_id,),
        )
        .await?;
    Ok(row.map(|(payment_id,)| payment_id))
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
