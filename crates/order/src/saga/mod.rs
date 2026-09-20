//! The `order-fulfillment` process manager: an orchestration saga that drives
//! one order from placement to shipment across inventory, payment and
//! shipping, and compensates when stock or payment falls through.
//!
//! State lives in the `OrderFulfillment` aggregate; each transition is one
//! subscription handler in its own file. Handlers are idempotent — they load
//! the saga, check its status and rely on the upstream commands' own
//! idempotency (deterministic ids, status guards) — so a redelivery after a
//! partial failure converges. An order whose code covered the whole total
//! skips the payment leg: it is settled and goes straight to shipping. An
//! order cancelled from outside (an operator, the customer) is compensated
//! the same way as a failed one, captured money included. The subscription is deliberately not strict:
//! it listens to a subset of four aggregates' events.

mod on_order_cancelled;
mod on_order_placed;
mod on_payment_captured;
mod on_payment_declined;
mod on_shipment_dispatched;
mod on_stock_reservation_rejected;
mod on_stock_reserved;

use evento::{
    Executor, Projection, ProjectionAggregate, metadata::Event, subscription::SubscriptionBuilder,
};
use timada_core::Money;
use timada_inventory::StockLocation;
use timada_payment::PaymentStatus;
use timada_shipping::{CreateShipment, DeliveryMethod, ShipmentLine, ShippingError};

use crate::{
    aggregator::{
        FulfillmentCompensated, FulfillmentCompleted, FulfillmentStarted, LineStockReserved,
        OrderFulfillment, PaymentCaptured, PaymentRequested, PaymentWaived, ShipmentRequested,
    },
    command::Command,
    error::OrderError,
    query::load_order_details,
    value_object::{FulfillmentLine, FulfillmentStatus, PaymentMode},
};

pub const ORDER_FULFILLMENT_SUBSCRIPTION: &str = "order-fulfillment";

pub fn fulfillment_id(order_id: &str) -> String {
    timada_core::id::derived(&[order_id], "fulfillment")
}

pub fn order_fulfillment_subscription<E: Executor>() -> SubscriptionBuilder<E> {
    SubscriptionBuilder::new(ORDER_FULFILLMENT_SUBSCRIPTION)
        .handler(on_order_placed::start_fulfillment())
        .handler(on_order_cancelled::compensate_cancelled_order())
        .handler(on_stock_reserved::record_line_reserved())
        .handler(on_stock_reservation_rejected::compensate_out_of_stock())
        .handler(on_payment_captured::request_shipment())
        .handler(on_payment_declined::compensate_declined_payment())
        .handler(on_shipment_dispatched::complete_fulfillment())
}

#[evento::projection(id = id)]
#[evento::snapshot(none)]
pub struct FulfillmentState {
    pub id: String,
    pub order_id: String,
    pub lines: Vec<FulfillmentLine>,
    pub reserved_product_ids: Vec<String>,
    pub pickup_store_id: Option<String>,
    pub amount: Money,
    pub payment_mode: PaymentMode,
    pub status: FulfillmentStatus,
    pub payment_id: Option<String>,
    pub shipment_id: Option<String>,
}

impl FulfillmentState {
    /// Where this order's stock is taken from.
    pub fn stock_location(&self) -> StockLocation {
        stock_location(self.pickup_store_id.as_deref())
    }

    pub fn all_reserved(&self) -> bool {
        self.lines
            .iter()
            .all(|l| self.reserved_product_ids.contains(&l.product_id))
    }

    /// The line whose stock item has the given aggregate id, if any.
    pub fn line_for_stock_item(&self, stock_item_id: &str) -> Option<&FulfillmentLine> {
        let location = self.stock_location();
        self.lines
            .iter()
            .find(|l| timada_inventory::stock_item_id(&l.product_id, &location) == stock_item_id)
    }
}

pub fn stock_location(pickup_store_id: Option<&str>) -> StockLocation {
    match pickup_store_id {
        Some(store_id) => StockLocation::Store {
            store_id: store_id.to_owned(),
        },
        None => StockLocation::Warehouse,
    }
}

pub fn create_projection<E: Executor>() -> Projection<E, FulfillmentState> {
    Projection::new::<OrderFulfillment>()
        .handler(on_fulfillment_started())
        .handler(on_line_stock_reserved())
        .handler(on_payment_requested())
        .skip::<PaymentCaptured>()
        .skip::<PaymentWaived>()
        .handler(on_shipment_requested())
        .handler(on_fulfillment_completed())
        .handler(on_fulfillment_compensated())
        .strict()
}

/// By the saga's own aggregate id, for handlers of its events.
pub(crate) async fn load_fulfillment_by_id<E: Executor>(
    executor: &E,
    fulfillment_id: &str,
) -> anyhow::Result<Option<FulfillmentState>> {
    create_projection()
        .load(fulfillment_id)
        .execute(executor)
        .await
}

pub async fn load_fulfillment<E: Executor>(
    executor: &E,
    order_id: &str,
) -> anyhow::Result<Option<FulfillmentState>> {
    create_projection()
        .load(fulfillment_id(order_id))
        .execute(executor)
        .await
}

/// Hands the order's lines to shipping. Shipment creation is idempotent (id
/// derived from the order), so a retry returns the same shipment.
async fn create_shipment<E: Executor>(
    executor: &E,
    saga: &FulfillmentState,
) -> anyhow::Result<String> {
    let Some(order) = load_order_details(executor, &saga.order_id).await? else {
        anyhow::bail!("order {} missing while fulfilling", saga.order_id);
    };
    let method = DeliveryMethod::resolve(
        &order.delivery.method_code,
        order.delivery.pickup_store_id.clone(),
    )
    .ok_or_else(|| OrderError::UnknownDeliveryMethod(order.delivery.method_code.clone()))?;
    let shipment_id = timada_shipping::Command(executor)
        .create_shipment(CreateShipment {
            order_id: saga.order_id.clone(),
            method,
            destination: order.delivery_address,
            lines: saga
                .lines
                .iter()
                .map(|l| ShipmentLine {
                    product_id: l.product_id.clone(),
                    quantity: l.quantity,
                })
                .collect(),
        })
        .await?;
    Ok(shipment_id)
}

/// Gives back whatever was captured for the order and not refunded yet. The
/// amount is what is left at that moment, so a retry refunds nothing twice.
async fn refund_captured<E: Executor>(
    executor: &E,
    saga: &FulfillmentState,
    reason: &str,
) -> anyhow::Result<()> {
    let Some(payment_id) = &saga.payment_id else {
        return Ok(());
    };
    let Some(payment) = timada_payment::load_payment(executor, payment_id).await? else {
        return Ok(());
    };
    if payment.status != PaymentStatus::Captured {
        return Ok(());
    }
    // Refunds already on their way to the provider are not asked for again.
    let left = payment.refundable()?;
    if !left.is_positive() {
        return Ok(());
    }
    timada_payment::Command(executor)
        .refund_payment(payment_id, left, format!("order cancelled: {reason}"))
        .await?;
    tracing::info!(order_id = %saga.order_id, "captured payment refunded");
    Ok(())
}

/// Stops the parcel that was waiting for the carrier. One that already left
/// cannot be called back: that is logged and the rest of the compensation
/// goes on.
async fn cancel_pending_shipment<E: Executor>(
    executor: &E,
    saga: &FulfillmentState,
    reason: &str,
) -> anyhow::Result<()> {
    let Some(shipment_id) = &saga.shipment_id else {
        return Ok(());
    };
    match timada_shipping::Command(executor)
        .cancel_shipment(shipment_id, format!("order cancelled: {reason}"))
        .await
    {
        Ok(()) => Ok(()),
        Err(ShippingError::NotCreated) => {
            tracing::warn!(order_id = %saga.order_id, "order cancelled after its parcel left");
            Ok(())
        }
        Err(err) => Err(err.into()),
    }
}

/// Releases every reservation this saga holds, stops the pending shipment,
/// refunds what was captured, cancels the order and closes the saga. Every
/// step is idempotent, so a retry after a crash converges.
async fn compensate<E: Executor>(
    executor: &E,
    saga: &FulfillmentState,
    reason: &str,
) -> anyhow::Result<()> {
    let inventory = timada_inventory::Command(executor);
    let location = saga.stock_location();
    for product_id in &saga.reserved_product_ids {
        inventory
            .release_stock(
                timada_inventory::stock_item_id(product_id, &location),
                &saga.order_id,
            )
            .await?;
    }
    cancel_pending_shipment(executor, saga, reason).await?;
    refund_captured(executor, saga, reason).await?;
    Command(executor)
        .cancel_order(&saga.order_id, reason)
        .await?;
    saga.write()?
        .event(&FulfillmentCompensated {
            reason: reason.to_owned(),
        })
        .commit(executor)
        .await?;
    tracing::warn!(order_id = %saga.order_id, %reason, "order fulfillment compensated");
    Ok(())
}

#[evento::handler]
async fn on_fulfillment_started(
    event: Event<FulfillmentStarted>,
    row: &mut FulfillmentState,
) -> anyhow::Result<()> {
    row.id = event.aggregate_id.to_owned();
    row.order_id = event.data.order_id;
    row.lines = event.data.lines;
    row.pickup_store_id = event.data.pickup_store_id;
    row.amount = event.data.amount;
    row.payment_mode = event.data.payment_mode;
    row.status = FulfillmentStatus::ReservingStock;
    Ok(())
}

#[evento::handler]
async fn on_line_stock_reserved(
    event: Event<LineStockReserved>,
    row: &mut FulfillmentState,
) -> anyhow::Result<()> {
    row.reserved_product_ids.push(event.data.product_id);
    Ok(())
}

#[evento::handler]
async fn on_payment_requested(
    event: Event<PaymentRequested>,
    row: &mut FulfillmentState,
) -> anyhow::Result<()> {
    row.payment_id = Some(event.data.payment_id);
    row.status = FulfillmentStatus::AwaitingPayment;
    Ok(())
}

#[evento::handler]
async fn on_shipment_requested(
    event: Event<ShipmentRequested>,
    row: &mut FulfillmentState,
) -> anyhow::Result<()> {
    row.shipment_id = Some(event.data.shipment_id);
    row.status = FulfillmentStatus::AwaitingShipment;
    Ok(())
}

#[evento::handler]
async fn on_fulfillment_completed(
    _event: Event<FulfillmentCompleted>,
    row: &mut FulfillmentState,
) -> anyhow::Result<()> {
    row.status = FulfillmentStatus::Completed;
    Ok(())
}

#[evento::handler]
async fn on_fulfillment_compensated(
    _event: Event<FulfillmentCompensated>,
    row: &mut FulfillmentState,
) -> anyhow::Result<()> {
    row.status = FulfillmentStatus::Compensated;
    Ok(())
}
