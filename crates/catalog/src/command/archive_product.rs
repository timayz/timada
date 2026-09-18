use evento::{Executor, ProjectionAggregate};

use crate::{aggregator::ProductArchived, error::CatalogError};

impl<E: Executor> super::Command<E> {
    pub async fn archive_product(&self, id: impl Into<String>) -> Result<(), CatalogError> {
        let product = self.load_active(id).await?;

        product
            .write()?
            .event(&ProductArchived)
            .commit(&self.0)
            .await?;
        tracing::info!(product_id = %product.id, "product archived");
        Ok(())
    }
}
