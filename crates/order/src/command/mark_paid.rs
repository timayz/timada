use evento::{Executor, ProjectionAggregate};

use crate::{aggregator::OrderPaid, error::OrderError, value_object::OrderStatus};

impl<E: Executor> super::Command<'_, E> {
    /// Records the captured payment. Idempotent for the same payment so the
    /// fulfillment saga can retry.
    pub async fn mark_paid(
        &self,
        id: impl Into<String>,
        payment_id: impl Into<String>,
    ) -> Result<(), OrderError> {
        let payment_id = payment_id.into();
        let order = self.load_existing(id).await?;
        if order.status == OrderStatus::Paid && order.payment_id.as_deref() == Some(&payment_id) {
            return Ok(());
        }
        order.expect_status(OrderStatus::Placed)?;

        order
            .write()?
            .event(&OrderPaid { payment_id })
            .commit(self.0)
            .await?;
        tracing::info!(order_id = %order.id, "order paid");
        Ok(())
    }
}
