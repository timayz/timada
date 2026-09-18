use evento::{Executor, ProjectionAggregate};

use crate::{aggregator::ProductEnergyLabelled, error::CatalogError, value_object::EnergyClass};

impl<E: Executor> super::Command<E> {
    pub async fn label_product_energy(
        &self,
        id: impl Into<String>,
        class: EnergyClass,
        info_sheet_url: String,
    ) -> Result<(), CatalogError> {
        if info_sheet_url.trim().is_empty() {
            return Err(CatalogError::Required("info_sheet_url"));
        }
        let product = self.load_active(id).await?;

        product
            .write()?
            .event(&ProductEnergyLabelled {
                class,
                info_sheet_url,
            })
            .commit(&self.0)
            .await?;
        Ok(())
    }
}
