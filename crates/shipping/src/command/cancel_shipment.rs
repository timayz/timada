use evento::{Executor, ProjectionAggregate};

use crate::{aggregator::ShipmentCancelled, error::ShippingError, value_object::ShipmentStatus};

impl<E: Executor> super::Command<'_, E> {
    /// Cancels a shipment that has not left yet. A no-op when already
    /// cancelled so compensations can be retried; refused once the carrier
    /// has the parcel.
    pub async fn cancel_shipment(
        &self,
        id: impl Into<String>,
        reason: impl Into<String>,
    ) -> Result<(), ShippingError> {
        let shipment = self.load_existing(id).await?;
        match shipment.status {
            ShipmentStatus::Cancelled => return Ok(()),
            ShipmentStatus::Dispatched | ShipmentStatus::Delivered => {
                return Err(ShippingError::NotCreated);
            }
            ShipmentStatus::Created => {}
        }

        let reason = reason.into();
        shipment
            .write()?
            .event(&ShipmentCancelled {
                reason: reason.clone(),
            })
            .commit(self.0)
            .await?;
        tracing::info!(shipment_id = %shipment.id, %reason, "shipment cancelled");
        Ok(())
    }
}
