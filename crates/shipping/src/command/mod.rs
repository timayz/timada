mod create_shipment;
mod dispatch_shipment;
mod mark_delivered;

use std::ops::Deref;

pub use create_shipment::CreateShipment;

use evento::{Executor, Projection, metadata::Event};

use crate::{
    aggregator::{Shipment, ShipmentCreated, ShipmentDelivered, ShipmentDispatched},
    error::ShippingError,
    value_object::ShipmentStatus,
};

/// Deterministic shipment id: one shipment per order.
pub fn shipment_id(order_id: &str) -> String {
    timada_core::id::derived(&[order_id], "shipment")
}

pub struct Command<'a, E: Executor>(pub &'a E);

impl<E: Executor> Deref for Command<'_, E> {
    type Target = E;

    fn deref(&self) -> &Self::Target {
        self.0
    }
}

impl<E: Executor> Command<'_, E> {
    pub async fn load(&self, id: impl Into<String>) -> anyhow::Result<Option<ShipmentState>> {
        create_projection().load(id).execute(self.0).await
    }

    async fn load_existing(&self, id: impl Into<String>) -> Result<ShipmentState, ShippingError> {
        self.load(id).await?.ok_or(ShippingError::ShipmentNotFound)
    }
}

/// Write-side state: just enough to guard the commands.
#[evento::projection(id = id)]
#[evento::snapshot(none)]
pub struct ShipmentState {
    pub id: String,
    pub order_id: String,
    pub status: ShipmentStatus,
}

// Strict: a non-strict projection only *reads* the events it handles, so the
// version it observes (and `write()` relies on) would go stale.
fn create_projection<E: Executor>() -> Projection<E, ShipmentState> {
    Projection::new::<Shipment>()
        .handler(on_shipment_created())
        .handler(on_shipment_dispatched())
        .handler(on_shipment_delivered())
        .strict()
}

#[evento::handler]
async fn on_shipment_created(
    event: Event<ShipmentCreated>,
    row: &mut ShipmentState,
) -> anyhow::Result<()> {
    row.id = event.aggregate_id.to_owned();
    row.order_id = event.data.order_id;
    row.status = ShipmentStatus::Created;
    Ok(())
}

#[evento::handler]
async fn on_shipment_dispatched(
    _event: Event<ShipmentDispatched>,
    row: &mut ShipmentState,
) -> anyhow::Result<()> {
    row.status = ShipmentStatus::Dispatched;
    Ok(())
}

#[evento::handler]
async fn on_shipment_delivered(
    _event: Event<ShipmentDelivered>,
    row: &mut ShipmentState,
) -> anyhow::Result<()> {
    row.status = ShipmentStatus::Delivered;
    Ok(())
}
