use evento::{Executor, ProjectionAggregate};

use crate::{aggregator::BackInStockAlertRequested, error::InventoryError};

use super::alert_id;

#[derive(Debug, Clone)]
pub struct RequestBackInStockAlert {
    pub product_id: String,
    pub customer_id: String,
    pub email: String,
}

#[evento::command]
impl<E: Executor> super::Command<'_, E> {
    /// Registers a customer's "alerte disponibilité" for a product. One per
    /// product and customer, enforced by the derived id: asking again while
    /// it is pending is refused, asking again once it fired re-arms it.
    pub async fn request_back_in_stock_alert(
        &self,
        cmd: RequestBackInStockAlert,
        routing_key: Option<String>,
    ) -> Result<String, InventoryError> {
        if cmd.product_id.trim().is_empty() {
            return Err(InventoryError::Required("product_id"));
        }
        if cmd.customer_id.trim().is_empty() {
            return Err(InventoryError::Required("customer_id"));
        }
        if cmd.email.trim().is_empty() {
            return Err(InventoryError::Required("email"));
        }

        let id = alert_id(&cmd.product_id, &cmd.customer_id);
        if let Some(alert) = super::load_alert(self.0, &id).await? {
            if !alert.triggered {
                return Err(InventoryError::AlreadyRequested);
            }
            alert
                .write()?
                .event(&BackInStockAlertRequested {
                    product_id: cmd.product_id.clone(),
                    customer_id: cmd.customer_id,
                    email: cmd.email,
                })
                .commit(self.0)
                .await?;
            tracing::info!(alert_id = %id, product_id = %cmd.product_id, "back-in-stock alert re-armed");
            return Ok(id);
        }
        let result = evento::append(&id)
            .routing_key_opt(routing_key)
            .event(&BackInStockAlertRequested {
                product_id: cmd.product_id.clone(),
                customer_id: cmd.customer_id,
                email: cmd.email,
            })
            .commit(self.0)
            .await;

        match result {
            Ok(id) => {
                tracing::info!(alert_id = %id, product_id = %cmd.product_id, "back-in-stock alert requested");
                Ok(id)
            }
            Err(evento::WriteError::InvalidOriginalVersion) => {
                Err(InventoryError::AlreadyRequested)
            }
            Err(err) => Err(err.into()),
        }
    }
}
