//! The expanded order in "Historique de vos commandes": lines, delivery,
//! payment mode, fees and totals. Executor-backed snapshots via bitcode.

use evento::{Executor, metadata::Event, projection::Projection};
use timada_core::{Address, Money};

use crate::{
    aggregator::{
        Order, OrderCancelled, OrderConfirmationResent, OrderPaid, OrderPlaced, OrderShipped,
    },
    value_object::{DeliveryChoice, OrderLine, OrderStatus, PaymentMode, Seller, order_total},
};

#[evento::projection(bitcode::Encode, bitcode::Decode)]
#[derive(Debug, PartialEq)]
pub struct OrderDetailsView {
    pub id: String,
    pub cart_id: String,
    pub customer_id: String,
    pub seller: Seller,
    /// Unix seconds of `OrderPlaced`.
    pub placed_at: u64,
    pub status: OrderStatus,
    pub lines: Vec<OrderLine>,
    pub delivery_address: Address,
    pub billing_address: Address,
    pub delivery: DeliveryChoice,
    pub payment_mode: PaymentMode,
    pub shipping_fee: Money,
    pub handling_fee: Money,
    pub subtotal: Money,
    pub total: Money,
    pub promo_code: Option<String>,
    pub payment_id: Option<String>,
    pub shipment_id: Option<String>,
    pub carrier: Option<String>,
    pub tracking_number: Option<String>,
    /// Unix seconds of `OrderShipped` ("Expédiée le ...").
    pub shipped_at: Option<u64>,
    pub cancelled_reason: Option<String>,
    pub confirmation_resent_count: u32,
}

pub fn create_projection<E: Executor>() -> Projection<E, OrderDetailsView> {
    Projection::new::<Order>()
        .handler(on_order_placed())
        .handler(on_order_paid())
        .handler(on_order_shipped())
        .handler(on_order_cancelled())
        .handler(on_order_confirmation_resent())
        .strict()
}

pub async fn load<E: Executor>(
    executor: &E,
    id: impl Into<String>,
) -> anyhow::Result<Option<OrderDetailsView>> {
    create_projection().load(id).execute(executor).await
}

#[evento::handler]
async fn on_order_placed(
    event: Event<OrderPlaced>,
    row: &mut OrderDetailsView,
) -> anyhow::Result<()> {
    let totals = order_total(
        &event.data.lines,
        &event.data.shipping_fee,
        &event.data.handling_fee,
    )?;
    row.id = event.aggregate_id.to_owned();
    row.placed_at = event.timestamp;
    row.status = OrderStatus::Placed;
    row.cart_id = event.data.cart_id;
    row.customer_id = event.data.customer_id;
    row.seller = event.data.seller;
    row.lines = event.data.lines;
    row.delivery_address = event.data.delivery_address;
    row.billing_address = event.data.billing_address;
    row.delivery = event.data.delivery;
    row.payment_mode = event.data.payment_mode;
    row.shipping_fee = event.data.shipping_fee;
    row.handling_fee = event.data.handling_fee;
    row.subtotal = totals.subtotal;
    row.total = totals.total;
    row.promo_code = event.data.promo_code;
    Ok(())
}

#[evento::handler]
async fn on_order_paid(event: Event<OrderPaid>, row: &mut OrderDetailsView) -> anyhow::Result<()> {
    row.status = OrderStatus::Paid;
    row.payment_id = Some(event.data.payment_id);
    Ok(())
}

#[evento::handler]
async fn on_order_shipped(
    event: Event<OrderShipped>,
    row: &mut OrderDetailsView,
) -> anyhow::Result<()> {
    row.status = OrderStatus::Shipped;
    row.shipped_at = Some(event.timestamp);
    row.shipment_id = Some(event.data.shipment_id);
    row.carrier = Some(event.data.carrier);
    row.tracking_number = Some(event.data.tracking_number);
    Ok(())
}

#[evento::handler]
async fn on_order_cancelled(
    event: Event<OrderCancelled>,
    row: &mut OrderDetailsView,
) -> anyhow::Result<()> {
    row.status = OrderStatus::Cancelled;
    row.cancelled_reason = Some(event.data.reason);
    Ok(())
}

#[evento::handler]
async fn on_order_confirmation_resent(
    _event: Event<OrderConfirmationResent>,
    row: &mut OrderDetailsView,
) -> anyhow::Result<()> {
    row.confirmation_resent_count += 1;
    Ok(())
}
