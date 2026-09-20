//! Everything the order page shows about a shipment, folded from one
//! `Shipment` stream. Executor-backed snapshots via the bitcode derives.

use evento::{Executor, metadata::Event, projection::Projection};
use timada_core::Address;

use crate::{
    aggregator::{
        Shipment, ShipmentCancelled, ShipmentCreated, ShipmentDelivered, ShipmentDispatched,
        ShipmentReplacesReturn,
    },
    value_object::{DeliveryMethod, ShipmentLine, ShipmentStatus},
};

#[evento::projection(bitcode::Encode, bitcode::Decode)]
#[derive(Debug, PartialEq)]
pub struct ShipmentView {
    pub id: String,
    pub order_id: String,
    pub method: DeliveryMethod,
    pub destination: Address,
    pub lines: Vec<ShipmentLine>,
    pub status: ShipmentStatus,
    pub carrier: Option<String>,
    pub tracking_number: Option<String>,
    pub cancelled_reason: Option<String>,
    /// The return this parcel is the replacement for; `None` for the order's
    /// own parcel.
    pub replaces_return: Option<String>,
}

pub fn create_projection<E: Executor>() -> Projection<E, ShipmentView> {
    Projection::new::<Shipment>()
        .handler(on_shipment_created())
        .handler(on_shipment_dispatched())
        .handler(on_shipment_delivered())
        .handler(on_shipment_cancelled())
        .handler(on_shipment_replaces_return())
        // The view gained `cancelled_reason` and a status, then
        // `replaces_return`: snapshots taken with a previous shape must not
        // be decoded.
        .revision(2)
        .strict()
}

pub async fn load<E: Executor>(
    executor: &E,
    id: impl Into<String>,
) -> anyhow::Result<Option<ShipmentView>> {
    create_projection().load(id).execute(executor).await
}

#[evento::handler]
async fn on_shipment_created(
    event: Event<ShipmentCreated>,
    row: &mut ShipmentView,
) -> anyhow::Result<()> {
    row.id = event.aggregate_id.to_owned();
    row.order_id = event.data.order_id;
    row.method = event.data.method;
    row.destination = event.data.destination;
    row.lines = event.data.lines;
    row.status = ShipmentStatus::Created;
    Ok(())
}

#[evento::handler]
async fn on_shipment_dispatched(
    event: Event<ShipmentDispatched>,
    row: &mut ShipmentView,
) -> anyhow::Result<()> {
    row.carrier = Some(event.data.carrier);
    row.tracking_number = Some(event.data.tracking_number);
    row.status = ShipmentStatus::Dispatched;
    Ok(())
}

#[evento::handler]
async fn on_shipment_delivered(
    _event: Event<ShipmentDelivered>,
    row: &mut ShipmentView,
) -> anyhow::Result<()> {
    row.status = ShipmentStatus::Delivered;
    Ok(())
}

#[evento::handler]
async fn on_shipment_cancelled(
    event: Event<ShipmentCancelled>,
    row: &mut ShipmentView,
) -> anyhow::Result<()> {
    row.status = ShipmentStatus::Cancelled;
    row.cancelled_reason = Some(event.data.reason);
    Ok(())
}

#[evento::handler]
async fn on_shipment_replaces_return(
    event: Event<ShipmentReplacesReturn>,
    row: &mut ShipmentView,
) -> anyhow::Result<()> {
    row.replaces_return = Some(event.data.reference);
    Ok(())
}
