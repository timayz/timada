use evento::{Executor, ProjectionAggregate};

use crate::{aggregator::StockReceived, error::InventoryError};

impl<E: Executor> super::Command<E> {
    pub async fn receive_stock(
        &self,
        id: impl Into<String>,
        quantity: u32,
    ) -> Result<(), InventoryError> {
        if quantity == 0 {
            return Err(InventoryError::InvalidQuantity);
        }
        let item = self.require_stock_item(id).await?;

        item.write()?
            .event(&StockReceived { quantity })
            .commit(&self.0)
            .await?;
        Ok(())
    }
}
