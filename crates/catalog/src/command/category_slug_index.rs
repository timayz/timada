use evento::ProjectionAggregate;
use evento::{Executor, WriteError};

use crate::aggregator::category::{Claimed, Released};
use crate::error::CatalogError;
use crate::value_object::category_slug_id;

use super::Command;

impl<E: Executor> Command<E> {
    /// Points a normalized slug at `category_id`, creating or re-claiming the
    /// index entry. Fails when it already points at another category.
    pub(crate) async fn claim_category_slug(
        &self,
        slug: &str,
        category_id: &str,
        routing_key: Option<String>,
    ) -> Result<(), CatalogError> {
        let claimed = Claimed {
            category_id: category_id.to_owned(),
        };
        let created = evento::append(category_slug_id(slug))
            .original_version(0)
            .routing_key_opt(routing_key)
            .event(&claimed)
            .commit(&self.0)
            .await;
        match created {
            Ok(_) => Ok(()),
            Err(WriteError::InvalidOriginalVersion) => {
                let Some(existing) = self.load_category_slug_index_any(slug).await? else {
                    return Err(CatalogError::SlugTaken);
                };
                if existing.released {
                    existing.write()?.event(&claimed).commit(&self.0).await?;
                    Ok(())
                } else if existing.category_id == category_id {
                    Ok(())
                } else {
                    Err(CatalogError::SlugTaken)
                }
            }
            Err(err) => Err(err.into()),
        }
    }

    /// Frees the index entry of a slug owned by `category_id`. A missing or
    /// already freed entry is fine; one owned by another category is left
    /// alone.
    pub(crate) async fn release_category_slug(
        &self,
        slug: &str,
        category_id: &str,
        routing_key: Option<String>,
    ) -> Result<(), CatalogError> {
        if let Some(index) = self.load_category_slug_index(slug).await?
            && index.category_id == category_id
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
