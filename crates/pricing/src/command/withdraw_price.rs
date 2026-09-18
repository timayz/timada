use evento::{Executor, ProjectionAggregate};

use crate::{aggregator::ProductPriceWithdrawn, error::PricingError};

impl<E: Executor> super::Command<E> {
    pub async fn withdraw_price(&self, id: impl Into<String>) -> Result<(), PricingError> {
        let price = self.load_active(id).await?;

        price
            .write()?
            .event(&ProductPriceWithdrawn)
            .commit(&self.0)
            .await?;
        tracing::info!(price_id = %price.id, "product price withdrawn");
        Ok(())
    }
}
