use evento::ProjectionAggregate;
use evento::{Executor, WriteError};
use timada_core::claim::claim_is_fresh;

use crate::aggregator::sku::{Claimed, Released};
use crate::error::CatalogError;
use crate::value_object::product_sku_id;

use super::{CLAIM_ATTEMPTS, Command, ProductSkuState};

impl<E: Executor> Command<E> {
    /// Points a stock code at one variant, at the version of the product
    /// its next write is made at. The entry is created, or taken over when
    /// it is free, already the variant's (the claim is renewed), or held by
    /// a variant that let go of it. Fails when another variant holds it, or
    /// wins the race for it.
    pub(crate) async fn claim_sku(
        &self,
        sku: &str,
        product_id: &str,
        variant_id: &str,
        product_version: u16,
        routing_key: Option<String>,
    ) -> Result<(), CatalogError> {
        let claimed = Claimed {
            product_id: product_id.to_owned(),
            variant_id: variant_id.to_owned(),
            product_version,
        };
        let created = evento::append(product_sku_id(sku))
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
            let Some(existing) = self.load_sku_index_any(sku).await? else {
                return Err(CatalogError::SkuTaken);
            };
            let free = existing.released
                || (existing.product_id == product_id && existing.variant_id == variant_id)
                || self.sku_let_go(sku, &existing).await?;
            if !free {
                return Err(CatalogError::SkuTaken);
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
        Err(CatalogError::SkuTaken)
    }

    /// Whether the variant an entry names has let go of the code: its
    /// product archived or deleted, or written past the claim without the
    /// variant taking the code (changed, removed, never defined), or never
    /// written at all once the claim is no longer fresh.
    async fn sku_let_go(&self, sku: &str, index: &ProductSkuState) -> anyhow::Result<bool> {
        let Some(product) = self.load_product_any(&index.product_id).await? else {
            return Ok(!claim_is_fresh(index.claimed_at, timada_core::unix_now()));
        };
        if product.is_archived() {
            return Ok(true);
        }
        if product.holds_sku(&index.variant_id, sku) {
            return Ok(false);
        }
        Ok(product.aggregate_version()? > index.product_version
            || !claim_is_fresh(index.claimed_at, timada_core::unix_now()))
    }

    /// Gives back a claim the variant could not use: the product's write
    /// after the claim at `product_version` failed. Left alone when the
    /// variant holds the code after all.
    pub(crate) async fn release_sku_claim(
        &self,
        sku: &str,
        product_id: &str,
        variant_id: &str,
        product_version: u16,
        routing_key: Option<String>,
    ) -> Result<(), CatalogError> {
        let held = self
            .load_product_any(product_id)
            .await?
            .is_some_and(|product| product.holds_sku(variant_id, sku));
        if held {
            return Ok(());
        }
        self.release_sku(
            sku,
            product_id,
            variant_id,
            product_version.saturating_add(1),
            routing_key,
        )
        .await
    }

    /// Frees the entry of a code the variant claimed before its product's
    /// version `before` and no longer holds (the caller checks that). An
    /// entry already free, another variant's, or claimed again since is
    /// left alone, so a variant taking a code back keeps it.
    pub(crate) async fn release_sku(
        &self,
        sku: &str,
        product_id: &str,
        variant_id: &str,
        before: u16,
        routing_key: Option<String>,
    ) -> Result<(), CatalogError> {
        for _ in 0..CLAIM_ATTEMPTS {
            let Some(index) = self.load_sku_index(sku).await? else {
                return Ok(());
            };
            if index.product_id != product_id
                || index.variant_id != variant_id
                || index.product_version >= before
            {
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
