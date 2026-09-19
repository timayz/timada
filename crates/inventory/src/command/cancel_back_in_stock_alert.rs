use evento::{Executor, ProjectionAggregate};

use crate::{aggregator::BackInStockAlertCancelled, error::InventoryError};

impl<E: Executor> super::Command<'_, E> {
    /// The customer no longer wants to be told. Only a pending alert is
    /// cancelled: one that already fired, or was already cancelled, is left
    /// as it is. Someone else's alert is reported as not found.
    pub async fn cancel_back_in_stock_alert(
        &self,
        id: impl Into<String>,
        customer_id: &str,
    ) -> Result<(), InventoryError> {
        let Some(alert) = super::load_alert(self.0, id).await? else {
            return Err(InventoryError::AlertNotFound);
        };
        if alert.customer_id != customer_id {
            return Err(InventoryError::AlertNotFound);
        }
        if !alert.is_pending() {
            return Ok(());
        }

        alert
            .write()?
            .event(&BackInStockAlertCancelled)
            .commit(self.0)
            .await?;
        tracing::info!(alert_id = %alert.id, product_id = %alert.product_id, "back-in-stock alert cancelled");
        Ok(())
    }
}
