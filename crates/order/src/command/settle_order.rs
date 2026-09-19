use evento::{Executor, ProjectionAggregate};

use crate::{
    aggregator::OrderSettled, error::OrderError, query::load_order_details,
    value_object::OrderStatus,
};

impl<E: Executor> super::Command<'_, E> {
    /// Counts an order with nothing left to pay as paid. Refused while an
    /// amount is due; idempotent so the fulfillment saga can retry.
    pub async fn settle_order(&self, id: impl Into<String>) -> Result<(), OrderError> {
        let order = self.load_existing(id).await?;
        if order.status == OrderStatus::Paid && order.payment_id.is_none() {
            return Ok(());
        }
        order.expect_status(OrderStatus::Placed)?;
        let Some(details) = load_order_details(self.0, &order.id).await? else {
            return Err(OrderError::OrderNotFound);
        };
        if details.total.is_positive() {
            return Err(OrderError::AmountDue { due: details.total });
        }

        order.write()?.event(&OrderSettled).commit(self.0).await?;
        tracing::info!(order_id = %order.id, "order settled without payment");
        Ok(())
    }
}
