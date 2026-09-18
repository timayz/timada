use evento::{Executor, ProjectionAggregate};

use crate::{aggregator::ShipmentDelivered, error::ShippingError, value_object::ShipmentStatus};

impl<E: Executor> super::Command<E> {
    pub async fn mark_delivered(&self, id: impl Into<String>) -> Result<(), ShippingError> {
        let shipment = self.load_existing(id).await?;
        if shipment.status != ShipmentStatus::Dispatched {
            return Err(ShippingError::NotDispatched);
        }

        shipment
            .write()?
            .event(&ShipmentDelivered)
            .commit(&self.0)
            .await?;
        Ok(())
    }
}
