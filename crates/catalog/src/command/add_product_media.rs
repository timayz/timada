use evento::{Executor, ProjectionAggregate};

use crate::{aggregator::ProductMediaAdded, error::CatalogError, value_object::Media};

impl<E: Executor> super::Command<'_, E> {
    pub async fn add_product_media(
        &self,
        id: impl Into<String>,
        media: Media,
    ) -> Result<(), CatalogError> {
        if media.url.trim().is_empty() {
            return Err(CatalogError::Required("media.url"));
        }
        let product = self.load_active(id).await?;

        product
            .write()?
            .event(&ProductMediaAdded { media })
            .commit(self.0)
            .await?;
        Ok(())
    }
}
