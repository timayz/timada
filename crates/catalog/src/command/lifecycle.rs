use evento::{Executor, ProjectionAggregate};

use crate::aggregator::{Archived, Deleted, Published, Unpublished};
use crate::command::ProductState;
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
            .tenant(&self.tenant)
            .commit(&self.executor)
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
            .tenant(&self.tenant)
            .commit(&self.executor)
            .await?;
        Ok(())
    }

    /// Retires the product for good. Its slug and the codes of its variants
    /// are free again at once to any claim, and the release subscription
    /// frees their entries. Already archived is a no-op.
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
            .routing_key_opt(routing_key)
            .event(&Archived {
                archived_by: actor.to_owned(),
            })
            .tenant(&self.tenant)
            .commit(&self.executor)
            .await?;
        Ok(())
    }

    /// Deletes the product for good: it leaves every listing, its slug and
    /// the codes of its variants are free again (the release subscription
    /// frees their entries), and the host deletes its pictures' files.
    /// Archived or not, any product can go; one that is gone already is
    /// `NotFound`. Only a product nobody ever ordered may be deleted, and
    /// that is the host's to check before calling: the catalog cannot see
    /// the orders. Returns what the product was, for the host to clean up
    /// after it.
    pub async fn delete_product(
        &self,
        actor: &str,
        product_id: &str,
        routing_key: Option<String>,
    ) -> Result<ProductState, CatalogError> {
        let product = self
            .load_product(product_id)
            .await?
            .ok_or(CatalogError::NotFound)?;
        product
            .write()?
            .routing_key_opt(routing_key)
            .event(&Deleted {
                deleted_by: actor.to_owned(),
            })
            .tenant(&self.tenant)
            .commit(&self.executor)
            .await?;
        Ok(product)
    }
}
