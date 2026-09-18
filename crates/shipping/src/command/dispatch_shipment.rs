use evento::{Executor, ProjectionAggregate};

use crate::{aggregator::ShipmentDispatched, error::ShippingError, value_object::ShipmentStatus};

impl<E: Executor> super::Command<E> {
    pub async fn dispatch_shipment(
        &self,
        id: impl Into<String>,
        carrier: String,
        tracking_number: String,
    ) -> Result<(), ShippingError> {
        if tracking_number.trim().is_empty() {
            return Err(ShippingError::Required("tracking_number"));
        }
        let shipment = self.load_existing(id).await?;
        if shipment.status != ShipmentStatus::Created {
            return Err(ShippingError::NotCreated);
        }

        shipment
            .write()?
            .event(&ShipmentDispatched {
                carrier,
                tracking_number: tracking_number.clone(),
            })
            .commit(&self.0)
            .await?;
        tracing::info!(shipment_id = %shipment.id, %tracking_number, "shipment dispatched");
        Ok(())
    }
}
