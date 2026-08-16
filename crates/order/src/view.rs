//! The write-side view of an order, replayed from its events.
//!
//! This is the read model for both the customer's order-status page and the
//! admin order detail — never the eventually-consistent `admin_order_list`
//! table, which exists only to list orders cheaply. Replaying through the `Rw`
//! executor means a customer redirected out of checkout sees the order that was
//! just written, and it means the fulfillment saga always decides against the
//! very latest state.

use evento::ProjectionAggregate;
use evento::metadata::Event;
use evento::projection::Projection;
use timada_core::{Executor, Money};

use crate::aggregate::{
    Address, Order, OrderCancelled, OrderDelivered, OrderForwardedToSupplier, OrderLine, OrderPaid,
    OrderPlaced, OrderShipped,
};

/// How far fulfillment has got.
///
/// The ordering is the saga's ratchet: a step only ever appends an event that
/// moves the status *up*, so a redelivered event is a no-op. `Cancelled` sorts
/// last deliberately — it is terminal, so an ordinary `status < Shipped` guard
/// also excludes a cancelled order without needing a second check. Cancelling
/// itself is the one transition that does not follow the ranking, and every
/// site that appends `OrderCancelled` states its own rule.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default, bitcode::Encode, bitcode::Decode,
)]
pub enum OrderStatus {
    #[default]
    Placed,
    Paid,
    Forwarded,
    Shipped,
    Delivered,
    Cancelled,
}

impl OrderStatus {
    /// Lowercase name, used by the admin read model and the status badges.
    pub fn as_str(&self) -> &'static str {
        match self {
            OrderStatus::Placed => "placed",
            OrderStatus::Paid => "paid",
            OrderStatus::Forwarded => "forwarded",
            OrderStatus::Shipped => "shipped",
            OrderStatus::Delivered => "delivered",
            OrderStatus::Cancelled => "cancelled",
        }
    }
}

/// One order, rebuilt from its event stream.
#[evento::projection(bitcode::Encode, bitcode::Decode)]
#[derive(Debug, PartialEq, Eq)]
pub struct OrderView {
    pub id: String,
    pub cart_id: String,
    pub email: String,
    pub shipping_address: Address,
    pub lines: Vec<OrderLine>,
    /// Tax-inclusive: what the customer pays, and what is charged.
    pub total: Money,
    /// The assessed split of [`total`](Self::total): `total_net + total_tax`
    /// equals it.
    pub total_net: Money,
    pub total_tax: Money,
    pub status: OrderStatus,
    /// `Some` once the charge was captured.
    pub payment_id: Option<String>,
    /// One entry per supplier that accepted its share of the lines.
    pub supplier_order_ids: Vec<String>,
    /// `Some` once a parcel is on its way.
    pub tracking_number: Option<String>,
    /// `Some` only once cancelled.
    pub cancel_reason: Option<String>,
}

impl ProjectionAggregate for OrderView {
    fn aggregate_id(&self) -> String {
        self.id.to_owned()
    }
}

impl OrderView {
    /// Has fulfillment got at least as far as `stage`?
    ///
    /// A cancelled order has reached nothing: the timeline stops and the page
    /// shows the cancellation instead.
    fn reached(&self, stage: OrderStatus) -> bool {
        self.status != OrderStatus::Cancelled && self.status >= stage
    }

    pub fn is_paid(&self) -> bool {
        self.reached(OrderStatus::Paid)
    }

    pub fn is_forwarded(&self) -> bool {
        self.reached(OrderStatus::Forwarded)
    }

    pub fn is_shipped(&self) -> bool {
        self.reached(OrderStatus::Shipped)
    }

    pub fn is_delivered(&self) -> bool {
        self.reached(OrderStatus::Delivered)
    }

    pub fn is_cancelled(&self) -> bool {
        self.status == OrderStatus::Cancelled
    }
}

#[evento::handler]
async fn apply_placed(event: Event<OrderPlaced>, view: &mut OrderView) -> anyhow::Result<()> {
    view.id = event.aggregate_id.clone();
    view.cart_id = event.data.cart_id.clone();
    view.email = event.data.email.clone();
    view.shipping_address = event.data.shipping_address.clone();
    view.lines = event.data.lines.clone();
    view.total = event.data.total;
    view.total_net = event.data.total_net;
    view.total_tax = event.data.total_tax;
    view.status = OrderStatus::Placed;
    Ok(())
}

#[evento::handler]
async fn apply_paid(event: Event<OrderPaid>, view: &mut OrderView) -> anyhow::Result<()> {
    view.payment_id = Some(event.data.payment_id.clone());
    view.status = OrderStatus::Paid;
    Ok(())
}

/// A multi-supplier order records every supplier order but only advances the
/// status once — "forwarded" means the order left our hands, not that every
/// supplier answered.
#[evento::handler]
async fn apply_forwarded(
    event: Event<OrderForwardedToSupplier>,
    view: &mut OrderView,
) -> anyhow::Result<()> {
    if !view
        .supplier_order_ids
        .contains(&event.data.supplier_order_id)
    {
        view.supplier_order_ids
            .push(event.data.supplier_order_id.clone());
    }
    view.status = OrderStatus::Forwarded;
    Ok(())
}

#[evento::handler]
async fn apply_shipped(event: Event<OrderShipped>, view: &mut OrderView) -> anyhow::Result<()> {
    view.tracking_number = Some(event.data.tracking_number.clone());
    view.status = OrderStatus::Shipped;
    Ok(())
}

#[evento::handler]
async fn apply_delivered(
    _event: Event<OrderDelivered>,
    view: &mut OrderView,
) -> anyhow::Result<()> {
    view.status = OrderStatus::Delivered;
    Ok(())
}

#[evento::handler]
async fn apply_cancelled(event: Event<OrderCancelled>, view: &mut OrderView) -> anyhow::Result<()> {
    view.cancel_reason = Some(event.data.reason.clone());
    view.status = OrderStatus::Cancelled;
    Ok(())
}

/// Replay one order. `None` means no such aggregate.
pub async fn load_order(executor: &Executor, order_id: &str) -> anyhow::Result<Option<OrderView>> {
    Projection::<_, OrderView>::new::<Order>()
        .handler(apply_placed())
        .handler(apply_paid())
        .handler(apply_forwarded())
        .handler(apply_shipped())
        .handler(apply_delivered())
        .handler(apply_cancelled())
        .strict()
        .load(order_id)
        .execute(executor)
        .await
}
