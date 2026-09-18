use evento::Executor;

use crate::{aggregator::StockItemRegistered, error::InventoryError, value_object::StockLocation};

use super::stock_item_id;

#[derive(Debug, Clone)]
pub struct RegisterStockItem {
    pub product_id: String,
    pub location: StockLocation,
}

#[evento::command]
impl<E: Executor> super::Command<E> {
    /// Starts tracking a product at a location. The id is derived from both,
    /// so registering the same pair twice is rejected atomically by the store.
    pub async fn register_stock_item(
        &self,
        cmd: RegisterStockItem,
        routing_key: Option<String>,
    ) -> Result<String, InventoryError> {
        if cmd.product_id.trim().is_empty() {
            return Err(InventoryError::Required("product_id"));
        }
        if let StockLocation::Store { store_id } = &cmd.location
            && store_id.trim().is_empty()
        {
            return Err(InventoryError::Required("location.store_id"));
        }

        let id = stock_item_id(&cmd.product_id, &cmd.location);
        let result = evento::append(&id)
            .routing_key_opt(routing_key)
            .event(&StockItemRegistered {
                product_id: cmd.product_id.clone(),
                location: cmd.location,
            })
            .commit(&self.0)
            .await;

        match result {
            Ok(id) => {
                tracing::info!(stock_item_id = %id, product_id = %cmd.product_id, "stock item registered");
                Ok(id)
            }
            Err(evento::WriteError::InvalidOriginalVersion) => {
                Err(InventoryError::AlreadyRegistered)
            }
            Err(err) => Err(err.into()),
        }
    }
}
