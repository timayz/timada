use evento::Executor;

use crate::{aggregator::ProductCreated, error::CatalogError, value_object::Brand};

use super::product_id;

#[derive(Debug, Clone)]
pub struct CreateProduct {
    pub sku: String,
    pub name: String,
    pub brand: Brand,
    pub category_path: Vec<String>,
    pub short_description: String,
    pub warranty_months: u16,
}

#[evento::command]
impl<E: Executor> super::Command<'_, E> {
    /// Enters a product into the catalog. The id is derived from the SKU, so
    /// a second product with the same SKU is rejected atomically by the store.
    pub async fn create_product(
        &self,
        cmd: CreateProduct,
        routing_key: Option<String>,
    ) -> Result<String, CatalogError> {
        let sku = cmd.sku.trim().to_uppercase();
        if sku.is_empty() {
            return Err(CatalogError::Required("sku"));
        }
        if cmd.name.trim().is_empty() {
            return Err(CatalogError::Required("name"));
        }
        if cmd.brand.slug.trim().is_empty() {
            return Err(CatalogError::Required("brand.slug"));
        }

        let id = product_id(&sku);
        let result = evento::append(&id)
            .routing_key_opt(routing_key)
            .event(&ProductCreated {
                sku: sku.clone(),
                name: cmd.name,
                brand: cmd.brand,
                category_path: cmd.category_path,
                short_description: cmd.short_description,
                warranty_months: cmd.warranty_months,
            })
            .commit(self.0)
            .await;

        match result {
            Ok(id) => {
                tracing::info!(product_id = %id, %sku, "product created");
                Ok(id)
            }
            Err(evento::WriteError::InvalidOriginalVersion) => {
                Err(CatalogError::SkuAlreadyExists(sku))
            }
            Err(err) => Err(err.into()),
        }
    }
}
