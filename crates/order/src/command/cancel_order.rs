use evento::{Executor, ProjectionAggregate};

use crate::{aggregator::OrderCancelled, error::OrderError, value_object::OrderStatus};

impl<E: Executor> super::Command<'_, E> {
    /// Cancels an order that has not shipped. A no-op when already cancelled
    /// so compensations can be retried.
    pub async fn cancel_order(
        &self,
        id: impl Into<String>,
        reason: impl Into<String>,
    ) -> Result<(), OrderError> {
        let order = self.load_existing(id).await?;
        match order.status {
            OrderStatus::Cancelled => return Ok(()),
            OrderStatus::Shipped => {
                return Err(OrderError::WrongStatus {
                    expected: "placed or paid",
                    actual: order.status.as_str(),
                });
            }
            OrderStatus::Placed | OrderStatus::Paid => {}
        }

        let reason = reason.into();
        order
            .write()?
            .event(&OrderCancelled {
                reason: reason.clone(),
            })
            .commit(self.0)
            .await?;
        tracing::info!(order_id = %order.id, %reason, "order cancelled");
        Ok(())
    }
}
