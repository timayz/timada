use evento::{Executor, ProjectionAggregate};

use crate::{aggregator::ProductSpecified, error::CatalogError, value_object::Spec};

impl<E: Executor> super::Command<'_, E> {
    /// Replaces the whole "fiche technique".
    pub async fn specify_product(
        &self,
        id: impl Into<String>,
        specs: Vec<Spec>,
    ) -> Result<(), CatalogError> {
        let product = self.load_active(id).await?;

        product
            .write()?
            .event(&ProductSpecified { specs })
            .commit(self.0)
            .await?;
        Ok(())
    }
}
