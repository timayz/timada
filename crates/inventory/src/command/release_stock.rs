use evento::{Executor, ProjectionAggregate};

use crate::{aggregator::StockReservationReleased, error::InventoryError};

impl<E: Executor> super::Command<'_, E> {
    /// Gives an order's reservation back. A no-op when the order holds none,
    /// so compensations can be retried freely.
    pub async fn release_stock(
        &self,
        id: impl Into<String>,
        order_id: impl Into<String>,
    ) -> Result<(), InventoryError> {
        let order_id = order_id.into();
        let item = self.require_stock_item(id).await?;

        let Some(quantity) = item.reservation_for(&order_id) else {
            return Ok(());
        };

        item.write()?
            .event(&StockReservationReleased { order_id, quantity })
            .commit(self.0)
            .await?;
        Ok(())
    }
}
