use evento::{Executor, ProjectionAggregate};

use crate::{aggregator::BackInStockAlertTriggered, error::InventoryError};

impl<E: Executor> super::Command<'_, E> {
    /// Marks the alert as sent. A no-op once triggered, or when cancelled.
    pub async fn trigger_back_in_stock_alert(
        &self,
        id: impl Into<String>,
    ) -> Result<(), InventoryError> {
        trigger_alert(self.0, id).await
    }
}

/// Shared by the command and the `inventory-back-in-stock` subscription,
/// which only holds a reference to the executor.
pub async fn trigger_alert<E: Executor>(
    executor: &E,
    id: impl Into<String>,
) -> Result<(), InventoryError> {
    let Some(alert) = super::load_alert(executor, id).await? else {
        return Err(InventoryError::AlertNotFound);
    };
    if !alert.is_pending() {
        return Ok(());
    }

    alert
        .write()?
        .event(&BackInStockAlertTriggered)
        .commit(executor)
        .await?;
    tracing::info!(alert_id = %alert.id, product_id = %alert.product_id, "back-in-stock alert triggered");
    Ok(())
}
