use evento::{Executor, ProjectionAggregate};

use crate::{aggregator::OrderShipped, error::OrderError, value_object::OrderStatus};

impl<E: Executor> super::Command<'_, E> {
    /// Records the carrier handover. Idempotent for the same shipment.
    pub async fn mark_shipped(
        &self,
        id: impl Into<String>,
        shipment_id: impl Into<String>,
        carrier: String,
        tracking_number: String,
    ) -> Result<(), OrderError> {
        let shipment_id = shipment_id.into();
        if tracking_number.trim().is_empty() {
            return Err(OrderError::Required("tracking_number"));
        }
        let order = self.load_existing(id).await?;
        if order.status == OrderStatus::Shipped
            && order.shipment_id.as_deref() == Some(&shipment_id)
        {
            return Ok(());
        }
        order.expect_status(OrderStatus::Paid)?;

        order
            .write()?
            .event(&OrderShipped {
                shipment_id,
                carrier,
                tracking_number,
            })
            .commit(self.0)
            .await?;
        tracing::info!(order_id = %order.id, "order shipped");
        Ok(())
    }
}
