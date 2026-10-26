//! The combinations a product sells. A variant names a value for every
//! option exactly once, so the menus on a product page always resolve to one
//! of them or to nothing.

use evento::{Executor, ProjectionAggregate};

use crate::aggregator::{
    VariantDefined, VariantImaged, VariantPriced, VariantRemoved, VariantSkuChanged,
};
use crate::error::CatalogError;
use crate::value_object::{
    MAX_AMOUNT_CENTS, MAX_VARIANTS, OptionSelection, new_part_id, normalize_currency, normalize_sku,
};

use super::ProductState;

/// A combination to sell: a value for every option, a price of its own, and
/// optionally the merchant's code for it and one of the product's pictures.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DefineVariant {
    pub selections: Vec<OptionSelection>,
    pub sku: Option<String>,
    pub amount_cents: u64,
    pub currency: String,
    pub image_id: Option<String>,
}

#[evento::command]
impl<E: Executor> super::Command<E> {
    /// Adds a combination to sell and returns its id. The code, when given,
    /// is claimed first and given back if the commit fails.
    pub async fn define_variant(
        &self,
        actor: &str,
        product_id: &str,
        cmd: DefineVariant,
        routing_key: Option<String>,
    ) -> Result<String, CatalogError> {
        let currency = normalize_currency(&cmd.currency)?;
        if cmd.amount_cents > MAX_AMOUNT_CENTS {
            return Err(CatalogError::InvalidAmount);
        }
        let sku = cmd.sku.as_deref().map(str::trim).filter(|s| !s.is_empty());
        let sku = sku.map(normalize_sku).transpose()?;
        let product = self.require_editable(product_id).await?;
        if product.variants.len() >= MAX_VARIANTS {
            return Err(CatalogError::TooManyVariants);
        }
        let selections = ordered_selections(&product, &cmd.selections)?;
        if product.variant_for(&selections).is_some() {
            return Err(CatalogError::VariantExists);
        }
        require_same_currency(&product, &currency)?;
        if let Some(image_id) = &cmd.image_id
            && !product.images.iter().any(|image| &image.id == image_id)
        {
            return Err(CatalogError::ImageNotFound);
        }

        let variant_id = new_part_id();
        if let Some(sku) = &sku {
            self.claim_sku(sku, product_id, &variant_id, routing_key.clone())
                .await?;
        }
        let defined = product
            .write()?
            .routing_key_opt(routing_key.clone())
            .event(&VariantDefined {
                variant_id: variant_id.clone(),
                selections,
                sku: sku.clone(),
                amount_cents: cmd.amount_cents,
                currency,
                image_id: cmd.image_id,
                defined_by: actor.to_owned(),
            })
            .tenant(&self.tenant)
            .commit(&self.executor)
            .await;
        if let Err(err) = defined {
            if let Some(sku) = &sku
                && let Err(release) = self.release_sku(sku, &variant_id, routing_key).await
            {
                tracing::warn!(%sku, %release, "cannot give the code back after a failed variant");
            }
            return Err(err.into());
        }
        Ok(variant_id)
    }

    /// Changes what one combination costs; the same price is a no-op.
    pub async fn price_variant(
        &self,
        actor: &str,
        product_id: &str,
        variant_id: &str,
        amount_cents: u64,
        currency: &str,
        routing_key: Option<String>,
    ) -> Result<(), CatalogError> {
        let currency = normalize_currency(currency)?;
        if amount_cents > MAX_AMOUNT_CENTS {
            return Err(CatalogError::InvalidAmount);
        }
        let product = self.require_editable(product_id).await?;
        let variant = product
            .variant(variant_id)
            .ok_or(CatalogError::VariantNotFound)?;
        if variant.price.amount_cents == amount_cents && variant.price.currency == currency {
            return Ok(());
        }
        require_same_currency(&product, &currency)?;
        product
            .write()?
            .routing_key_opt(routing_key)
            .event(&VariantPriced {
                variant_id: variant_id.to_owned(),
                amount_cents,
                currency,
                priced_by: actor.to_owned(),
            })
            .tenant(&self.tenant)
            .commit(&self.executor)
            .await?;
        Ok(())
    }

    /// Sets or clears the merchant's code for one combination. The new code
    /// is claimed before the change and the old one given back after.
    pub async fn change_variant_sku(
        &self,
        actor: &str,
        product_id: &str,
        variant_id: &str,
        sku: Option<&str>,
        routing_key: Option<String>,
    ) -> Result<(), CatalogError> {
        let sku = sku.map(str::trim).filter(|s| !s.is_empty());
        let sku = sku.map(normalize_sku).transpose()?;
        let product = self.require_editable(product_id).await?;
        let variant = product
            .variant(variant_id)
            .ok_or(CatalogError::VariantNotFound)?;
        let previous = variant.sku.clone();
        if previous == sku {
            return Ok(());
        }
        if let Some(sku) = &sku {
            self.claim_sku(sku, product_id, variant_id, routing_key.clone())
                .await?;
        }
        product
            .write()?
            .routing_key_opt(routing_key.clone())
            .event(&VariantSkuChanged {
                variant_id: variant_id.to_owned(),
                sku: sku.clone(),
                changed_by: actor.to_owned(),
            })
            .tenant(&self.tenant)
            .commit(&self.executor)
            .await?;
        if let Some(previous) = previous
            && let Err(release) = self.release_sku(&previous, variant_id, routing_key).await
        {
            tracing::warn!(sku = %previous, %release, "cannot give the old code back");
        }
        Ok(())
    }

    /// Picks which of the product's pictures a combination shows, or none.
    pub async fn image_variant(
        &self,
        actor: &str,
        product_id: &str,
        variant_id: &str,
        image_id: Option<&str>,
        routing_key: Option<String>,
    ) -> Result<(), CatalogError> {
        let image_id = image_id.map(str::trim).filter(|i| !i.is_empty());
        let product = self.require_editable(product_id).await?;
        let variant = product
            .variant(variant_id)
            .ok_or(CatalogError::VariantNotFound)?;
        if variant.image_id.as_deref() == image_id {
            return Ok(());
        }
        if let Some(image_id) = image_id
            && !product.images.iter().any(|image| image.id == image_id)
        {
            return Err(CatalogError::ImageNotFound);
        }
        product
            .write()?
            .routing_key_opt(routing_key)
            .event(&VariantImaged {
                variant_id: variant_id.to_owned(),
                image_id: image_id.map(str::to_owned),
                imaged_by: actor.to_owned(),
            })
            .tenant(&self.tenant)
            .commit(&self.executor)
            .await?;
        Ok(())
    }

    /// Stops selling a combination and gives its code back. One that is not
    /// there is a no-op.
    pub async fn remove_variant(
        &self,
        actor: &str,
        product_id: &str,
        variant_id: &str,
        routing_key: Option<String>,
    ) -> Result<(), CatalogError> {
        let product = self.require_editable(product_id).await?;
        let Some(variant) = product.variant(variant_id) else {
            return Ok(());
        };
        let sku = variant.sku.clone();
        product
            .write()?
            .routing_key_opt(routing_key.clone())
            .event(&VariantRemoved {
                variant_id: variant_id.to_owned(),
                removed_by: actor.to_owned(),
            })
            .tenant(&self.tenant)
            .commit(&self.executor)
            .await?;
        if let Some(sku) = sku
            && let Err(release) = self.release_sku(&sku, variant_id, routing_key).await
        {
            tracing::warn!(%sku, %release, "cannot give the code back after removing a variant");
        }
        Ok(())
    }
}

/// The selections in the product's own option order, once every option has
/// exactly one known value.
fn ordered_selections(
    product: &ProductState,
    selections: &[OptionSelection],
) -> Result<Vec<OptionSelection>, CatalogError> {
    if product.options.is_empty() || selections.len() != product.options.len() {
        return Err(CatalogError::InvalidSelection);
    }
    let mut ordered = Vec::with_capacity(product.options.len());
    for option in &product.options {
        let mut named = selections
            .iter()
            .filter(|selection| selection.option_id == option.id);
        let selection = named.next().ok_or(CatalogError::InvalidSelection)?;
        if named.next().is_some() || !option.values.contains(&selection.value) {
            return Err(CatalogError::InvalidSelection);
        }
        ordered.push(selection.clone());
    }
    Ok(ordered)
}

/// Everything priced on one product is priced in one currency: a range
/// across two of them would mean nothing.
fn require_same_currency(product: &ProductState, currency: &str) -> Result<(), CatalogError> {
    match product.currency() {
        Some(known) if known != currency => Err(CatalogError::CurrencyMismatch),
        _ => Ok(()),
    }
}
