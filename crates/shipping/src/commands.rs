//! Write-side commands for [`Shipment`](crate::aggregate::Shipment).

use evento::AggregateExt as _;
use evento::ProjectionAggregate as _;
use timada_core::Executor;
use timada_dropship::{SupplierRegistry, TrackingStatus};

use crate::aggregate::{ShipmentCreated, ShipmentDelivered, ShipmentDispatched};
use crate::view::load_shipment;

/// The id of the shipment for `(order_id, supplier_id)`.
///
/// Deriving it instead of generating one means the fulfillment saga can replay
/// `create_shipment` without keeping a mapping table, and a redelivered event
/// lands on the same aggregate. This pass models one parcel per
/// `(order, supplier)`; split shipments would need the supplier's own parcel
/// reference in the id.
pub fn shipment_id(order_id: &str, supplier_id: &str) -> String {
    evento::hash_ids(vec![order_id, supplier_id])
}

/// Start tracking a parcel for one order and supplier.
///
/// Idempotent on `ShipmentCreated`: calling it twice for the same
/// `(order_id, supplier_id)` returns the existing id and writes nothing.
#[tracing::instrument(skip(executor))]
pub async fn create_shipment(
    executor: &Executor,
    order_id: &str,
    supplier_id: &str,
    external_ref: &str,
) -> anyhow::Result<String> {
    let id = shipment_id(order_id, supplier_id);

    if executor.has_event::<ShipmentCreated>(&id).await? {
        tracing::debug!(shipment_id = %id, "shipment already created, skipping");
        return Ok(id);
    }

    evento::append(&id)
        .event(&ShipmentCreated {
            order_id: order_id.to_owned(),
            supplier_id: supplier_id.to_owned(),
            external_ref: external_ref.to_owned(),
        })
        .commit(executor)
        .await?;

    tracing::info!(shipment_id = %id, %supplier_id, "shipment created");
    Ok(id)
}

/// Ask the supplier where the parcel is and record any progress.
///
/// Writes **at most one** event per call: the supplier reports a state, not a
/// transition, so a status we have already recorded is a no-op. A delivered
/// shipment is never polled again. Missing shipments are logged rather than
/// raised — this runs from a button and from the saga, and neither should fail
/// because an id no longer resolves.
#[tracing::instrument(skip(executor, registry))]
pub async fn refresh_tracking(
    executor: &Executor,
    registry: &SupplierRegistry,
    shipment_id: &str,
) -> anyhow::Result<()> {
    let Some(view) = load_shipment(executor, shipment_id).await? else {
        tracing::warn!(%shipment_id, "no such shipment, nothing to refresh");
        return Ok(());
    };

    if view.delivered {
        tracing::debug!(%shipment_id, "shipment already delivered, not polling");
        return Ok(());
    }

    let status = registry
        .get(&view.supplier_id)?
        .track_shipment(&view.external_ref)
        .await?;

    match status {
        TrackingStatus::Dispatched {
            tracking_number,
            carrier,
        } if !view.dispatched => {
            tracing::info!(%shipment_id, %tracking_number, %carrier, "shipment dispatched");
            view.write()?
                .event(&ShipmentDispatched {
                    tracking_number,
                    carrier,
                })
                .commit(executor)
                .await?;
        }
        // `view.delivered` is false here — the early return above covers it.
        TrackingStatus::Delivered => {
            tracing::info!(%shipment_id, "shipment delivered");
            view.write()?
                .event(&ShipmentDelivered)
                .commit(executor)
                .await?;
        }
        TrackingStatus::Pending | TrackingStatus::Dispatched { .. } => {
            tracing::debug!(%shipment_id, "supplier reports no change");
        }
    }

    Ok(())
}
