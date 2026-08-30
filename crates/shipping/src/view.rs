//! The write-side view of a shipment, replayed from its events.
//!
//! Unlike [`crate::projections`] — the eventual-consistent SQL table that backs
//! the admin page — this is loaded on demand straight from the event store, so
//! a reader always sees every event committed so far. [`refresh_tracking`] uses
//! it to decide whether a supplier's answer is news, and the fulfillment saga
//! uses it to map a shipment aggregate id (all a `ShipmentDispatched` event
//! carries) back to the order it belongs to.
//!
//! [`refresh_tracking`]: crate::commands::refresh_tracking

use evento::ProjectionAggregate;
use evento::metadata::Event;
use evento::projection::Projection;
use timada_core::Executor;

use crate::aggregate::{Shipment, ShipmentCreated, ShipmentDelivered, ShipmentDispatched};

/// `dispatched` and `delivered` are independent flags rather than one status
/// enum because the aggregate accepts delivery without a prior dispatch.
#[evento::projection(bitcode::Encode, bitcode::Decode)]
#[derive(Debug, PartialEq, Eq)]
pub struct ShipmentView {
    pub id: String,
    pub order_id: String,
    pub supplier_id: String,
    /// The supplier's own reference for the parcel.
    pub external_ref: String,
    pub dispatched: bool,
    pub delivered: bool,
    /// `Some` only once dispatched.
    pub tracking_number: Option<String>,
    /// `Some` only once dispatched.
    pub carrier: Option<String>,
}

impl ProjectionAggregate for ShipmentView {
    fn aggregate_id(&self) -> String {
        self.id.to_owned()
    }
}

#[evento::handler]
async fn apply_created(
    event: Event<ShipmentCreated>,
    view: &mut ShipmentView,
) -> anyhow::Result<()> {
    view.id = event.aggregate_id.clone();
    view.order_id = event.data.order_id.clone();
    view.supplier_id = event.data.supplier_id.clone();
    view.external_ref = event.data.external_ref.clone();
    Ok(())
}

#[evento::handler]
async fn apply_dispatched(
    event: Event<ShipmentDispatched>,
    view: &mut ShipmentView,
) -> anyhow::Result<()> {
    view.dispatched = true;
    view.tracking_number = Some(event.data.tracking_number.clone());
    view.carrier = Some(event.data.carrier.clone());
    Ok(())
}

#[evento::handler]
async fn apply_delivered(
    _event: Event<ShipmentDelivered>,
    view: &mut ShipmentView,
) -> anyhow::Result<()> {
    view.delivered = true;
    Ok(())
}

/// Replay one shipment. `None` means no such aggregate.
pub async fn load_shipment(
    executor: &Executor,
    shipment_id: &str,
) -> anyhow::Result<Option<ShipmentView>> {
    Projection::<_, ShipmentView>::new::<Shipment>()
        .handler(apply_created())
        .handler(apply_dispatched())
        .handler(apply_delivered())
        .strict()
        .load(shipment_id)
        .execute(executor)
        .await
}
