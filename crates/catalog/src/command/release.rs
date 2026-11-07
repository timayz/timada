//! Giving back the slugs and stock codes a product or a category let go
//! of, for the release subscription ([`crate::release_subscription`]).

use std::collections::BTreeSet;

use evento::Executor;

use crate::aggregator::{Product, ProductEvent};
use crate::error::CatalogError;
use crate::value_object::sku_key;

use super::Command;

impl<E: Executor> Command<E> {
    /// Frees every slug and stock code the product claimed before its
    /// version `before` and holds no more: what archiving it, deleting it,
    /// renaming it or changing its variants gave up. The values come from
    /// its whole history, so one release missed earlier is made up for
    /// too. Told twice, nothing more happens; a value the product took back
    /// since is kept.
    pub(crate) async fn release_given_up(
        &self,
        product_id: &str,
        before: u16,
        routing_key: Option<String>,
    ) -> Result<(), CatalogError> {
        let Some(product) = self.load_product_any(product_id).await? else {
            return Ok(());
        };
        let mut slugs = BTreeSet::new();
        let mut skus = BTreeSet::new();
        let history = evento::read::<Product>(product_id)
            .tenant(&self.tenant)
            .decode(&self.executor)
            .await?;
        for event in history {
            match event {
                ProductEvent::Created(created) => {
                    slugs.insert(created.slug);
                }
                ProductEvent::Renamed(renamed) => {
                    slugs.insert(renamed.slug);
                }
                ProductEvent::VariantDefined(defined) => {
                    if let Some(sku) = defined.sku {
                        skus.insert((sku_key(&sku), defined.variant_id));
                    }
                }
                ProductEvent::VariantSkuChanged(changed) => {
                    if let Some(sku) = changed.sku {
                        skus.insert((sku_key(&sku), changed.variant_id));
                    }
                }
                _ => {}
            }
        }
        for slug in slugs.iter().filter(|slug| !product.holds_slug(slug)) {
            self.release_slug(slug, product_id, before, routing_key.clone())
                .await?;
        }
        for (sku, variant_id) in skus
            .iter()
            .filter(|(sku, variant_id)| !product.holds_sku(variant_id, sku))
        {
            self.release_sku(sku, product_id, variant_id, before, routing_key.clone())
                .await?;
        }
        Ok(())
    }

    /// Frees the slug of an archived category. Told twice, nothing more
    /// happens.
    pub(crate) async fn release_category_given_up(
        &self,
        category_id: &str,
        routing_key: Option<String>,
    ) -> Result<(), CatalogError> {
        let Some(category) = self.load_category(category_id).await? else {
            return Ok(());
        };
        self.release_category_slug(&category.slug, category_id, routing_key)
            .await
    }
}
