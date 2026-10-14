use evento::ProjectionAggregate;
use evento::{Executor, WriteError};

use crate::aggregator::sku::{Claimed, Released};
use crate::error::CatalogError;
use crate::value_object::product_sku_id;

use super::Command;

impl<E: Executor> Command<E> {
    /// Points a stock code at one variant, creating or re-claiming the index
    /// entry. Fails when it already points at another variant.
    pub(crate) async fn claim_sku(
        &self,
        sku: &str,
        product_id: &str,
        variant_id: &str,
        routing_key: Option<String>,
    ) -> Result<(), CatalogError> {
        let claimed = Claimed {
            product_id: product_id.to_owned(),
            variant_id: variant_id.to_owned(),
        };
        let created = evento::append(product_sku_id(sku))
            .original_version(0)
            .routing_key_opt(routing_key)
            .event(&claimed)
            .commit(&self.0)
            .await;
        match created {
            Ok(_) => Ok(()),
            Err(WriteError::InvalidOriginalVersion) => {
                let Some(existing) = self.load_sku_index_any(sku).await? else {
                    return Err(CatalogError::SkuTaken);
                };
                if existing.released {
                    existing.write()?.event(&claimed).commit(&self.0).await?;
                    Ok(())
                } else if existing.variant_id == variant_id {
                    Ok(())
                } else {
                    Err(CatalogError::SkuTaken)
                }
            }
            Err(err) => Err(err.into()),
        }
    }

    /// Frees the index entry of a code held by `variant_id`. A missing or
    /// already freed entry is fine; one held by another variant is left
    /// alone.
    pub(crate) async fn release_sku(
        &self,
        sku: &str,
        variant_id: &str,
        routing_key: Option<String>,
    ) -> Result<(), CatalogError> {
        if let Some(index) = self.load_sku_index(sku).await?
            && index.variant_id == variant_id
        {
            index
                .write()?
                .routing_key_opt(routing_key)
                .event(&Released)
                .commit(&self.0)
                .await?;
        }
        Ok(())
    }
}
