//! Write side: the command gateway and the strict write models it loads.
//!
//! Every write model folds *all* events of its aggregate (`strict()`), so the
//! version it observes, and `write()` sends as `original_version`, never goes
//! stale. None of them is snapshotted: a cart or an order is a handful of
//! events.

mod cart;
mod checkout;
mod lifecycle;

use std::fmt;
use std::ops::Deref;

pub use cart::*;
pub use checkout::*;

use evento::{Executor, Projection, metadata::Event};

use crate::aggregator::{
    Cancelled, CheckedOut, Claimed, Created, Fulfilled, ItemAdded, ItemRemoved, Paid, Placed,
    QuantitySet,
};
use crate::value_object::{OrderLine, ShippingAddress, order_id};

/// The command gateway. Cheap to clone when `E` is.
pub struct Command<E: Executor>(pub E);

impl<E: Executor> Deref for Command<E> {
    type Target = E;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

// =============================================================================
// Cart
// =============================================================================

/// One line of a cart: a product, or one variant of it, and how many.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CartLine {
    pub product_id: String,
    pub variant_id: Option<String>,
    pub quantity: u32,
}

impl CartLine {
    /// Whether this line is the one of that product and variant.
    pub fn is(&self, product_id: &str, variant_id: Option<&str>) -> bool {
        self.product_id == product_id && self.variant_id.as_deref() == variant_id
    }
}

#[evento::projection(id = id, name = "timada-orders/CartState")]
#[evento::snapshot(none)]
#[derive(Debug, PartialEq, Eq)]
pub struct CartState {
    pub id: String,
    /// In the order they were first added.
    pub lines: Vec<CartLine>,
    /// The order it became, once checked out.
    pub order_id: Option<String>,
    /// Unix seconds of `Created`.
    pub created_at: u64,
    /// Unix seconds of the last event.
    pub updated_at: u64,
}

impl CartState {
    pub fn is_checked_out(&self) -> bool {
        self.order_id.is_some()
    }

    /// How many items, all lines together.
    pub fn item_count(&self) -> u32 {
        self.lines.iter().map(|line| line.quantity).sum()
    }

    pub fn line(&self, product_id: &str, variant_id: Option<&str>) -> Option<&CartLine> {
        self.lines
            .iter()
            .find(|line| line.is(product_id, variant_id))
    }
}

pub fn cart_projection<E: Executor>() -> Projection<E, CartState> {
    Projection::new::<crate::aggregator::Cart>()
        .handler(on_cart_created())
        .handler(on_item_added())
        .handler(on_quantity_set())
        .handler(on_item_removed())
        .handler(on_checked_out())
        .strict()
}

#[evento::handler]
async fn on_cart_created(event: Event<Created>, cart: &mut CartState) -> anyhow::Result<()> {
    cart.id = event.aggregate_id.to_owned();
    cart.created_at = event.timestamp;
    cart.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_item_added(event: Event<ItemAdded>, cart: &mut CartState) -> anyhow::Result<()> {
    let variant_id = event.data.variant_id.as_deref();
    match cart
        .lines
        .iter_mut()
        .find(|line| line.is(&event.data.product_id, variant_id))
    {
        Some(line) => line.quantity += event.data.quantity,
        None => cart.lines.push(CartLine {
            product_id: event.data.product_id.to_owned(),
            variant_id: event.data.variant_id.to_owned(),
            quantity: event.data.quantity,
        }),
    }
    cart.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_quantity_set(event: Event<QuantitySet>, cart: &mut CartState) -> anyhow::Result<()> {
    let variant_id = event.data.variant_id.as_deref();
    if let Some(line) = cart
        .lines
        .iter_mut()
        .find(|line| line.is(&event.data.product_id, variant_id))
    {
        line.quantity = event.data.quantity;
    }
    cart.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_item_removed(event: Event<ItemRemoved>, cart: &mut CartState) -> anyhow::Result<()> {
    let variant_id = event.data.variant_id.as_deref();
    cart.lines
        .retain(|line| !line.is(&event.data.product_id, variant_id));
    cart.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_checked_out(event: Event<CheckedOut>, cart: &mut CartState) -> anyhow::Result<()> {
    cart.order_id = Some(event.data.order_id.to_owned());
    cart.updated_at = event.timestamp;
    Ok(())
}

// =============================================================================
// Order
// =============================================================================

/// Where an order stands. Placed orders wait for payment; the merchant marks
/// them paid, then fulfilled, or cancels them before they leave.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum OrderStatus {
    #[default]
    AwaitingPayment,
    Paid,
    Fulfilled,
    Cancelled,
}

impl OrderStatus {
    pub const ALL: [Self; 4] = [
        Self::AwaitingPayment,
        Self::Paid,
        Self::Fulfilled,
        Self::Cancelled,
    ];

    /// The value stored in the read models.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AwaitingPayment => "awaiting_payment",
            Self::Paid => "paid",
            Self::Fulfilled => "fulfilled",
            Self::Cancelled => "cancelled",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|status| status.as_str() == value)
    }

    /// How a page words it.
    pub fn label(self) -> &'static str {
        match self {
            Self::AwaitingPayment => "Awaiting payment",
            Self::Paid => "Paid",
            Self::Fulfilled => "Fulfilled",
            Self::Cancelled => "Cancelled",
        }
    }
}

impl fmt::Display for OrderStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[evento::projection(id = id, name = "timada-orders/OrderState")]
#[evento::snapshot(none)]
#[derive(Debug, PartialEq, Eq)]
pub struct OrderState {
    pub id: String,
    pub reference: String,
    pub cart_id: String,
    pub email: String,
    pub shipping: ShippingAddress,
    pub note: Option<String>,
    pub lines: Vec<OrderLine>,
    pub currency: String,
    pub total_cents: u64,
    pub status: OrderStatus,
    pub tracking: Option<String>,
    pub cancel_reason: Option<String>,
    /// Unix seconds of `Placed`.
    pub placed_at: u64,
    pub paid_at: Option<u64>,
    pub fulfilled_at: Option<u64>,
    pub cancelled_at: Option<u64>,
    /// Unix seconds of the last event.
    pub updated_at: u64,
}

impl OrderState {
    /// How many items, all lines together.
    pub fn item_count(&self) -> u32 {
        self.lines.iter().map(|line| line.quantity).sum()
    }
}

pub fn order_projection<E: Executor>() -> Projection<E, OrderState> {
    Projection::new::<crate::aggregator::Order>()
        .handler(on_placed())
        .handler(on_paid())
        .handler(on_fulfilled())
        .handler(on_cancelled())
        .strict()
}

#[evento::handler]
async fn on_placed(event: Event<Placed>, order: &mut OrderState) -> anyhow::Result<()> {
    order.id = event.aggregate_id.to_owned();
    order.reference = event.data.reference.to_owned();
    order.cart_id = event.data.cart_id.to_owned();
    order.email = event.data.email.to_owned();
    order.shipping = event.data.shipping.to_owned();
    order.note = event.data.note.to_owned();
    order.lines = event.data.lines.to_owned();
    order.currency = event.data.currency.to_owned();
    order.total_cents = event.data.total_cents;
    order.status = OrderStatus::AwaitingPayment;
    order.placed_at = event.timestamp;
    order.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_paid(event: Event<Paid>, order: &mut OrderState) -> anyhow::Result<()> {
    order.status = OrderStatus::Paid;
    order.paid_at = Some(event.timestamp);
    order.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_fulfilled(event: Event<Fulfilled>, order: &mut OrderState) -> anyhow::Result<()> {
    order.status = OrderStatus::Fulfilled;
    order.tracking = event.data.tracking.to_owned();
    order.fulfilled_at = Some(event.timestamp);
    order.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_cancelled(event: Event<Cancelled>, order: &mut OrderState) -> anyhow::Result<()> {
    order.status = OrderStatus::Cancelled;
    order.cancel_reason = Some(event.data.reason.to_owned());
    order.cancelled_at = Some(event.timestamp);
    order.updated_at = event.timestamp;
    Ok(())
}

// =============================================================================
// OrderReference (index)
// =============================================================================

#[evento::projection(id = id, name = "timada-orders/OrderReferenceState")]
#[evento::snapshot(none)]
#[derive(Debug, PartialEq, Eq)]
pub struct OrderReferenceState {
    pub id: String,
    pub order_id: String,
}

pub fn order_reference_projection<E: Executor>() -> Projection<E, OrderReferenceState> {
    Projection::new::<crate::aggregator::OrderReference>()
        .handler(on_reference_claimed())
        .strict()
}

#[evento::handler]
async fn on_reference_claimed(
    event: Event<Claimed>,
    index: &mut OrderReferenceState,
) -> anyhow::Result<()> {
    index.id = event.aggregate_id.to_owned();
    index.order_id = event.data.order_id.to_owned();
    Ok(())
}

// =============================================================================
// Loads
// =============================================================================

impl<E: Executor> Command<E> {
    pub async fn load_cart(&self, cart_id: &str) -> anyhow::Result<Option<CartState>> {
        cart_projection().load(cart_id).execute(&self.0).await
    }

    pub async fn load_order(&self, order_id: &str) -> anyhow::Result<Option<OrderState>> {
        order_projection().load(order_id).execute(&self.0).await
    }

    /// The order a cart was placed as, if it was.
    pub async fn load_order_of_cart(&self, cart_id: &str) -> anyhow::Result<Option<OrderState>> {
        self.load_order(&order_id(cart_id)).await
    }

    /// The cart, or `CartNotFound`.
    pub(crate) async fn require_cart(
        &self,
        cart_id: &str,
    ) -> Result<CartState, crate::OrdersError> {
        self.load_cart(cart_id)
            .await?
            .ok_or(crate::OrdersError::CartNotFound)
    }

    /// The order, or `OrderNotFound`.
    pub(crate) async fn require_order(
        &self,
        order_id: &str,
    ) -> Result<OrderState, crate::OrdersError> {
        self.load_order(order_id)
            .await?
            .ok_or(crate::OrdersError::OrderNotFound)
    }
}
