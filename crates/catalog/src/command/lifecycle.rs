use evento::{Executor, ProjectionAggregate};

use crate::aggregator::{Archived, Published, Unpublished};
use crate::error::CatalogError;

#[evento::command]
impl<E: Executor> super::Command<E> {
    /// Puts the product on sale. It needs a price of its own or at least one
    /// variant to pick; already published is a no-op.
    pub async fn publish_product(
        &self,
        actor: &str,
        product_id: &str,
        routing_key: Option<String>,
    ) -> Result<(), CatalogError> {
        let product = self.require_editable(product_id).await?;
        if product.is_published() {
            return Ok(());
        }
        if !product.is_priced() {
            return Err(CatalogError::NotPriced);
        }
        product
            .write()?
            .routing_key_opt(routing_key)
            .event(&Published {
                published_by: actor.to_owned(),
            })
            .commit(&self.0)
            .await?;
        Ok(())
    }

    /// Takes the product off sale, back to a draft; a draft is a no-op.
    pub async fn unpublish_product(
        &self,
        actor: &str,
        product_id: &str,
        routing_key: Option<String>,
    ) -> Result<(), CatalogError> {
        let product = self.require_editable(product_id).await?;
        if !product.is_published() {
            return Ok(());
        }
        product
            .write()?
            .routing_key_opt(routing_key)
            .event(&Unpublished {
                unpublished_by: actor.to_owned(),
            })
            .commit(&self.0)
            .await?;
        Ok(())
    }

    /// Retires the product for good and frees its slug and the codes of its
    /// variants. Already archived is a no-op.
    pub async fn archive_product(
        &self,
        actor: &str,
        product_id: &str,
        routing_key: Option<String>,
    ) -> Result<(), CatalogError> {
        let product = match self.require_editable(product_id).await {
            Ok(product) => product,
            Err(CatalogError::Archived) => return Ok(()),
            Err(err) => return Err(err),
        };
        product
            .write()?
            .routing_key_opt(routing_key.clone())
            .event(&Archived {
                archived_by: actor.to_owned(),
            })
            .commit(&self.0)
            .await?;
        if let Err(release) = self
            .release_slug(&product.slug, product_id, routing_key.clone())
            .await
        {
            tracing::warn!(slug = %product.slug, %release, "cannot give the slug back after archiving");
        }
        for variant in &product.variants {
            if let Some(sku) = &variant.sku
                && let Err(release) = self
                    .release_sku(sku, &variant.id, routing_key.clone())
                    .await
            {
                tracing::warn!(%sku, %release, "cannot give a code back after archiving");
            }
        }
        Ok(())
    }
}
