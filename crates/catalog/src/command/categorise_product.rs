use evento::{Executor, ProjectionAggregate};

use crate::{aggregator::ProductCategorised, error::CatalogError};

impl<E: Executor> super::Command<'_, E> {
    /// Files a product under a category that is open; filing it where it
    /// already is changes nothing. Returns whether it moved.
    pub async fn categorise_product(
        &self,
        id: impl Into<String>,
        category_id: impl Into<String>,
    ) -> Result<bool, CatalogError> {
        let category_id = category_id.into();
        let product = self.load_active(id).await?;
        if product.category_id.as_deref() == Some(&category_id) {
            return Ok(false);
        }
        self.load_open_category(&category_id).await?;

        product
            .write()?
            .event(&ProductCategorised { category_id })
            .commit(self.0)
            .await?;
        Ok(true)
    }
}
