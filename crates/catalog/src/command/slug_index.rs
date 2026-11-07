use evento::ProjectionAggregate;
use evento::{Executor, WriteError};
use timada_core::claim::claim_is_fresh;

use crate::aggregator::{Claimed, Released};
use crate::error::CatalogError;
use crate::value_object::{product_slug_id, suffixed_slug};

use super::{CLAIM_ATTEMPTS, Command, ProductSlugState};

/// Tries at the derived slug, then with fresh random suffixes.
const SLUG_ATTEMPTS: usize = 4;

impl<E: Executor> Command<E> {
    /// Claims the slug derived from a name for `product_id`, with a random
    /// suffix when it is taken, and returns the one it got.
    pub(crate) async fn claim_derived_slug(
        &self,
        base: String,
        product_id: &str,
        product_version: u16,
        routing_key: Option<String>,
    ) -> Result<String, CatalogError> {
        let mut slug = base.clone();
        for attempt in 0..SLUG_ATTEMPTS {
            match self
                .claim_slug(&slug, product_id, product_version, routing_key.clone())
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

    /// Points a normalized slug at `product_id`, at the version of the
    /// product its next write is made at. The entry is created, or taken
    /// over when it is free, already the product's (the claim is renewed,
    /// so a release of what the product gave up earlier leaves it alone),
    /// or held by a product that let go of it. Fails when another product
    /// holds it, or wins the race for it.
    pub(crate) async fn claim_slug(
        &self,
        slug: &str,
        product_id: &str,
        product_version: u16,
        routing_key: Option<String>,
    ) -> Result<(), CatalogError> {
        let claimed = Claimed {
            product_id: product_id.to_owned(),
            product_version,
        };
        let created = evento::append(product_slug_id(slug))
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
            let Some(existing) = self.load_slug_index_any(slug).await? else {
                return Err(CatalogError::SlugTaken);
            };
            let free = existing.released
                || existing.product_id == product_id
                || self.slug_let_go(slug, &existing).await?;
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

    /// Whether the product an entry names has let go of the slug: archived
    /// or deleted, written past its claim without taking the slug, or never
    /// written at all once the claim is no longer fresh.
    async fn slug_let_go(&self, slug: &str, index: &ProductSlugState) -> anyhow::Result<bool> {
        let Some(product) = self.load_product_any(&index.product_id).await? else {
            return Ok(!claim_is_fresh(index.claimed_at, timada_core::unix_now()));
        };
        if product.is_archived() {
            return Ok(true);
        }
        if product.holds_slug(slug) {
            return Ok(false);
        }
        Ok(product.aggregate_version()? > index.product_version
            || !claim_is_fresh(index.claimed_at, timada_core::unix_now()))
    }

    /// Gives back a claim the product could not use: its write after the
    /// claim at `product_version` failed, and the product has been written
    /// past that version without the slug. While it is still at that
    /// version the write may land yet (the store could not tell), so the
    /// claim stays: the next claim takes it over once the product writes
    /// past it, or once it is no longer fresh.
    pub(crate) async fn release_slug_claim(
        &self,
        slug: &str,
        product_id: &str,
        product_version: u16,
        routing_key: Option<String>,
    ) -> Result<(), CatalogError> {
        let Some(product) = self.load_product_any(product_id).await? else {
            return Ok(());
        };
        if product.holds_slug(slug) || product.aggregate_version()? <= product_version {
            return Ok(());
        }
        self.release_slug(
            slug,
            product_id,
            product_version.saturating_add(1),
            routing_key,
        )
        .await
    }

    /// Frees the entry of a slug the product claimed before its version
    /// `before` and no longer holds (the caller checks that). An entry
    /// already free, another product's, or claimed again since is left
    /// alone, so a product taking a slug back keeps it.
    pub(crate) async fn release_slug(
        &self,
        slug: &str,
        product_id: &str,
        before: u16,
        routing_key: Option<String>,
    ) -> Result<(), CatalogError> {
        for _ in 0..CLAIM_ATTEMPTS {
            let Some(index) = self.load_slug_index(slug).await? else {
                return Ok(());
            };
            if index.product_id != product_id || index.product_version >= before {
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

#[cfg(test)]
mod tests {
    use evento::ProjectionAggregate;

    use crate::aggregator::Renamed;
    use crate::{CatalogError, Command, CreateProduct, ProductState};

    async fn with_a_mug() -> (Command<evento::Turso>, ProductState) {
        let store = evento::Turso::in_memory_concurrent().await.unwrap();
        let cmd = Command::new(store, "acme");
        let mug = cmd
            .create_product(
                "alice",
                CreateProduct {
                    name: "Blue Mug".into(),
                    slug: None,
                },
            )
            .await
            .unwrap();
        (cmd, mug)
    }

    /// A rename claims its slug, and its write fails with an outcome the
    /// store could not tell: the compensation keeps the claim, so the write
    /// landing after all leaves the slug to the product alone.
    #[tokio::test]
    async fn a_claim_whose_write_may_still_land_is_kept() {
        let (cmd, mug) = with_a_mug().await;
        let version = mug.aggregate_version().unwrap();
        cmd.claim_slug("tea-cup", &mug.id, version, None)
            .await
            .unwrap();
        cmd.release_slug_claim("tea-cup", &mug.id, version, None)
            .await
            .unwrap();
        assert!(cmd.load_slug_index("tea-cup").await.unwrap().is_some());

        mug.write()
            .unwrap()
            .event(&Renamed {
                name: "Tea Cup".into(),
                slug: "tea-cup".into(),
                renamed_by: "alice".into(),
            })
            .tenant("acme")
            .commit(cmd.executor())
            .await
            .unwrap();
        let taken = cmd
            .create_product(
                "alice",
                CreateProduct {
                    name: "Tea Cup".into(),
                    slug: Some("tea-cup".into()),
                },
            )
            .await;
        assert!(matches!(taken, Err(CatalogError::SlugTaken)), "{taken:?}");
    }

    /// A write refused because another landed first never lands: once the
    /// product is past the claim, the compensation gives it back.
    #[tokio::test]
    async fn a_claim_whose_write_was_refused_is_given_back() {
        let (cmd, mug) = with_a_mug().await;
        let version = mug.aggregate_version().unwrap();
        cmd.claim_slug("tea-cup", &mug.id, version, None)
            .await
            .unwrap();
        cmd.describe_product("alice", &mug.id, "Holds tea.")
            .await
            .unwrap();
        cmd.release_slug_claim("tea-cup", &mug.id, version, None)
            .await
            .unwrap();
        assert!(cmd.load_slug_index("tea-cup").await.unwrap().is_none());
    }
}
