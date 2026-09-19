use evento::{Executor, ProjectionAggregate};

use crate::{aggregator::StockReturned, error::InventoryError};

impl<E: Executor> super::Command<'_, E> {
    /// Puts units a customer sent back into stock again. Idempotent per
    /// return: a process manager retrying after a crash adds nothing twice.
    pub async fn restock_return(
        &self,
        id: impl Into<String>,
        return_id: impl Into<String>,
        quantity: u32,
    ) -> Result<(), InventoryError> {
        if quantity == 0 {
            return Err(InventoryError::InvalidQuantity);
        }
        let return_id = return_id.into();
        if return_id.trim().is_empty() {
            return Err(InventoryError::Required("return_id"));
        }
        let item = self.require_stock_item(id).await?;
        if item.restocked_returns.contains(&return_id) {
            return Ok(());
        }

        item.write()?
            .event(&StockReturned {
                return_id,
                quantity,
            })
            .commit(self.0)
            .await?;
        Ok(())
    }
}
