//! A product's membership in categories.

use evento::{Executor, ProjectionAggregate};

use crate::aggregator::{Categorized, Uncategorized};
use crate::error::CatalogError;
use crate::value_object::MAX_CATEGORIES_PER_PRODUCT;

#[evento::command]
impl<E: Executor> super::Command<E> {
    /// Puts the product in a live category; already a member is a no-op.
    pub async fn categorize_product(
        &self,
        actor: &str,
        product_id: &str,
        category_id: &str,
        routing_key: Option<String>,
    ) -> Result<(), CatalogError> {
        let product = self.require_editable(product_id).await?;
        if product.categories.contains(category_id) {
            return Ok(());
        }
        if product.categories.len() >= MAX_CATEGORIES_PER_PRODUCT {
            return Err(CatalogError::TooManyCategories);
        }
        self.require_live_category(category_id).await?;
        product
            .write()?
            .routing_key_opt(routing_key)
            .event(&Categorized {
                category_id: category_id.to_owned(),
                categorized_by: actor.to_owned(),
            })
            .commit(&self.0)
            .await?;
        Ok(())
    }

    /// Takes the product out of a category; not a member is a no-op. The
    /// category may be archived by now: leaving it is always allowed.
    pub async fn uncategorize_product(
        &self,
        actor: &str,
        product_id: &str,
        category_id: &str,
        routing_key: Option<String>,
    ) -> Result<(), CatalogError> {
        let product = self.require_editable(product_id).await?;
        if !product.categories.contains(category_id) {
            return Ok(());
        }
        product
            .write()?
            .routing_key_opt(routing_key)
            .event(&Uncategorized {
                category_id: category_id.to_owned(),
                uncategorized_by: actor.to_owned(),
            })
            .commit(&self.0)
            .await?;
        Ok(())
    }
}
