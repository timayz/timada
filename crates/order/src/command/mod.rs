mod cancel_order;
mod mark_paid;
mod mark_shipped;
mod place_order;
mod resend_confirmation;
mod settle_order;

use std::ops::Deref;

pub use place_order::{OrderTax, PlaceOrder};

use evento::{Executor, Projection, metadata::Event};

use crate::{
    aggregator::{
        Order, OrderBuyerIdentified, OrderCancelled, OrderConfirmationResent, OrderDiscountApplied,
        OrderNumberAssigned, OrderPaid, OrderPlaced, OrderReverseCharged, OrderSettled,
        OrderShipped, OrderTaxed,
    },
    error::OrderError,
    value_object::OrderStatus,
};

/// Deterministic order id: one order per checked-out cart, which makes the
/// cart-checkout process manager idempotent.
pub fn order_id(cart_id: &str) -> String {
    timada_core::id::derived(&[cart_id], "order")
}

pub struct Command<'a, E: Executor>(pub &'a E);

impl<E: Executor> Deref for Command<'_, E> {
    type Target = E;

    fn deref(&self) -> &Self::Target {
        self.0
    }
}

impl<E: Executor> Command<'_, E> {
    pub async fn load(&self, id: impl Into<String>) -> anyhow::Result<Option<OrderState>> {
        create_projection().load(id).execute(self.0).await
    }

    async fn load_existing(&self, id: impl Into<String>) -> Result<OrderState, OrderError> {
        self.load(id).await?.ok_or(OrderError::OrderNotFound)
    }
}

/// Write-side state: just enough to guard the commands.
#[evento::projection(id = id)]
#[evento::snapshot(none)]
pub struct OrderState {
    pub id: String,
    pub status: OrderStatus,
    pub customer_id: String,
    pub payment_id: Option<String>,
    pub shipment_id: Option<String>,
}

impl OrderState {
    fn expect_status(&self, expected: OrderStatus) -> Result<(), OrderError> {
        if self.status == expected {
            Ok(())
        } else {
            Err(OrderError::WrongStatus {
                expected: expected.as_str(),
                actual: self.status.as_str(),
            })
        }
    }
}

// Strict + explicit skips: a non-strict projection only *reads* the events it
// handles, so the version it observes (and `write()` relies on) would go stale.
fn create_projection<E: Executor>() -> Projection<E, OrderState> {
    Projection::new::<Order>()
        .handler(on_order_placed())
        .handler(on_order_paid())
        .handler(on_order_settled())
        .handler(on_order_shipped())
        .handler(on_order_cancelled())
        .skip::<OrderNumberAssigned>()
        .skip::<OrderTaxed>()
        .skip::<OrderBuyerIdentified>()
        .skip::<OrderReverseCharged>()
        .skip::<OrderDiscountApplied>()
        .skip::<OrderConfirmationResent>()
        .strict()
}

#[evento::handler]
async fn on_order_placed(event: Event<OrderPlaced>, row: &mut OrderState) -> anyhow::Result<()> {
    row.id = event.aggregate_id.to_owned();
    row.customer_id = event.data.customer_id;
    row.status = OrderStatus::Placed;
    Ok(())
}

#[evento::handler]
async fn on_order_paid(event: Event<OrderPaid>, row: &mut OrderState) -> anyhow::Result<()> {
    row.status = OrderStatus::Paid;
    row.payment_id = Some(event.data.payment_id);
    Ok(())
}

#[evento::handler]
async fn on_order_settled(_event: Event<OrderSettled>, row: &mut OrderState) -> anyhow::Result<()> {
    row.status = OrderStatus::Paid;
    Ok(())
}

#[evento::handler]
async fn on_order_shipped(event: Event<OrderShipped>, row: &mut OrderState) -> anyhow::Result<()> {
    row.status = OrderStatus::Shipped;
    row.shipment_id = Some(event.data.shipment_id);
    Ok(())
}

#[evento::handler]
async fn on_order_cancelled(
    _event: Event<OrderCancelled>,
    row: &mut OrderState,
) -> anyhow::Result<()> {
    row.status = OrderStatus::Cancelled;
    Ok(())
}
