use evento::ProjectionAggregate;
use evento::{Executor, WriteError};
use timada_core::claim::claim_is_fresh;

use crate::aggregator::category::{Claimed, Released};
use crate::error::CatalogError;
use crate::value_object::category_slug_id;

use super::{CLAIM_ATTEMPTS, CategorySlugState, Command};

impl<E: Executor> Command<E> {
    /// Points a normalized slug at `category_id`. The entry is created, or
    /// taken over when it is free, already the category's, or held by a
    /// category that let go of it. Fails when another category holds it, or
    /// wins the race for it.
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
            .routing_key_opt(routing_key.clone())
            .event(&claimed)
            .tenant(&self.tenant)
            .commit(&self.executor)
            .await;
        match created {
            Ok(_) => return Ok(()),
            Err(WriteError::InvalidOriginalVersion) => {}
            Err(err) => return Err(err.into()),
        }
        for _ in 0..CLAIM_ATTEMPTS {
            let Some(existing) = self.load_category_slug_index_any(slug).await? else {
                return Err(CatalogError::SlugTaken);
            };
            let free = existing.released
                || existing.category_id == category_id
                || self.category_slug_let_go(&existing).await?;
            if !free {
                return Err(CatalogError::SlugTaken);
            }
            let taken = existing
                .write()?
                .routing_key_opt(routing_key.clone())
                .event(&claimed)
                .tenant(&self.tenant)
                .commit(&self.executor)
                .await;
            match taken {
                Ok(_) => return Ok(()),
                Err(WriteError::InvalidOriginalVersion) => continue,
                Err(err) => return Err(err.into()),
            }
        }
        Err(CatalogError::SlugTaken)
    }

    /// Whether the category an entry names has let go of the slug: archived,
    /// or never written once the claim is no longer fresh. A category keeps
    /// its slug for life.
    async fn category_slug_let_go(&self, index: &CategorySlugState) -> anyhow::Result<bool> {
        Ok(match self.load_category(&index.category_id).await? {
            Some(category) => category.archived,
            None => !claim_is_fresh(index.claimed_at, timada_core::unix_now()),
        })
    }

    /// Frees the entry of a slug owned by `category_id` once the category
    /// holds it no more: archived, or never written. A missing or already
    /// freed entry is fine; one owned by another category is left alone.
    pub(crate) async fn release_category_slug(
        &self,
        slug: &str,
        category_id: &str,
        routing_key: Option<String>,
    ) -> Result<(), CatalogError> {
        let held = self
            .load_category(category_id)
            .await?
            .is_some_and(|category| !category.archived);
        if held {
            return Ok(());
        }
        for _ in 0..CLAIM_ATTEMPTS {
            let Some(index) = self.load_category_slug_index(slug).await? else {
                return Ok(());
            };
            if index.category_id != category_id {
                return Ok(());
            }
            let released = index
                .write()?
                .routing_key_opt(routing_key.clone())
                .event(&Released)
                .tenant(&self.tenant)
                .commit(&self.executor)
                .await;
            match released {
                Ok(_) => return Ok(()),
                Err(WriteError::InvalidOriginalVersion) => continue,
                Err(err) => return Err(err.into()),
            }
        }
        Err(WriteError::InvalidOriginalVersion.into())
    }
}
