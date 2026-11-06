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
    /// after the claim at `product_version` failed, and the product has been
    /// written past that version without the variant taking the code. While
    /// it is still at that version the write may land yet (the store could
    /// not tell), so the claim stays: the next claim takes it over once the
    /// product writes past it, or once it is no longer fresh.
    pub(crate) async fn release_sku_claim(
        &self,
        sku: &str,
        product_id: &str,
        variant_id: &str,
        product_version: u16,
        routing_key: Option<String>,
    ) -> Result<(), CatalogError> {
        let Some(product) = self.load_product_any(product_id).await? else {
            return Ok(());
        };
        if product.holds_sku(variant_id, sku) || product.aggregate_version()? <= product_version {
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

#[cfg(test)]
mod tests {
    use evento::ProjectionAggregate;

    use crate::aggregator::VariantDefined;
    use crate::{CatalogError, Command, CreateProduct, DefineVariant, OptionSelection};

    /// A variant claims its code, and the product's write fails with an
    /// outcome the store could not tell: the claim stays, so the variant
    /// defined after all keeps the code to itself.
    #[tokio::test]
    async fn a_claim_whose_write_may_still_land_is_kept() {
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
        let colour = cmd
            .define_option(
                "alice",
                &mug.id,
                "Colour",
                vec!["Blue".into(), "Red".into()],
            )
            .await
            .unwrap();
        let mug = cmd.load_product(&mug.id).await.unwrap().unwrap();
        let version = mug.aggregate_version().unwrap();
        let pick = |value: &str| OptionSelection {
            option_id: colour.clone(),
            value: value.into(),
        };
        cmd.claim_sku("MUG-1", &mug.id, "v-blue", version, None)
            .await
            .unwrap();
        cmd.release_sku_claim("MUG-1", &mug.id, "v-blue", version, None)
            .await
            .unwrap();
        assert!(cmd.load_sku_index("MUG-1").await.unwrap().is_some());

        mug.write()
            .unwrap()
            .event(&VariantDefined {
                variant_id: "v-blue".into(),
                selections: vec![pick("Blue")],
                sku: Some("MUG-1".into()),
                amount_cents: 1250,
                currency: "EUR".into(),
                image_id: None,
                defined_by: "alice".into(),
            })
            .tenant("acme")
            .commit(cmd.executor())
            .await
            .unwrap();
        let taken = cmd
            .define_variant(
                "alice",
                &mug.id,
                DefineVariant {
                    selections: vec![pick("Red")],
                    sku: Some("mug-1".into()),
                    amount_cents: 1250,
                    currency: "EUR".into(),
                    image_id: None,
                },
            )
            .await;
        assert!(matches!(taken, Err(CatalogError::SkuTaken)), "{taken:?}");
    }
}
