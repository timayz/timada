use evento::ProjectionAggregate;
use evento::{Executor, WriteError};

use crate::aggregator::{Claimed, Released};
use crate::error::CatalogError;
use crate::value_object::{product_slug_id, suffixed_slug};

use super::Command;

/// Tries at the derived slug, then with fresh random suffixes.
const SLUG_ATTEMPTS: usize = 4;

impl<E: Executor> Command<E> {
    /// Claims the slug derived from a name for `product_id`, with a random
    /// suffix when it is taken, and returns the one it got.
    pub(crate) async fn claim_derived_slug(
        &self,
        base: String,
        product_id: &str,
        routing_key: Option<String>,
    ) -> Result<String, CatalogError> {
        let mut slug = base.clone();
        for attempt in 0..SLUG_ATTEMPTS {
            match self
                .claim_slug(&slug, product_id, routing_key.clone())
                .await
            {
                Ok(()) => break,
                Err(CatalogError::SlugTaken) if attempt + 1 < SLUG_ATTEMPTS => {
                    slug = suffixed_slug(&base);
                }
                Err(err) => return Err(err),
            }
        }
        Ok(slug)
    }

    /// Points a normalized slug at `product_id`, creating or re-claiming the
    /// index entry. Fails when it already points at another product.
    pub(crate) async fn claim_slug(
        &self,
        slug: &str,
        product_id: &str,
        routing_key: Option<String>,
    ) -> Result<(), CatalogError> {
        let claimed = Claimed {
            product_id: product_id.to_owned(),
        };
        let created = evento::append(product_slug_id(slug))
            .original_version(0)
            .routing_key_opt(routing_key)
            .event(&claimed)
            .tenant(&self.tenant)
            .commit(&self.executor)
            .await;
        match created {
            Ok(_) => Ok(()),
            Err(WriteError::InvalidOriginalVersion) => {
                let Some(existing) = self.load_slug_index_any(slug).await? else {
                    return Err(CatalogError::SlugTaken);
                };
                if existing.released {
                    existing
                        .write()?
                        .event(&claimed)
                        .tenant(&self.tenant)
                        .commit(&self.executor)
                        .await?;
                    Ok(())
                } else if existing.product_id == product_id {
                    Ok(())
                } else {
                    Err(CatalogError::SlugTaken)
                }
            }
            Err(err) => Err(err.into()),
        }
    }

    /// Frees the index entry of a slug owned by `product_id`. A missing or
    /// already freed entry is fine; one owned by another product is left
    /// alone.
    pub(crate) async fn release_slug(
        &self,
        slug: &str,
        product_id: &str,
        routing_key: Option<String>,
    ) -> Result<(), CatalogError> {
        if let Some(index) = self.load_slug_index(slug).await?
            && index.product_id == product_id
        {
            index
                .write()?
                .routing_key_opt(routing_key)
                .event(&Released)
                .tenant(&self.tenant)
                .commit(&self.executor)
                .await?;
        }
        Ok(())
    }
}
