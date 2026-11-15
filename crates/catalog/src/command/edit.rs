use evento::{Executor, ProjectionAggregate};

use crate::aggregator::{
    CompareAtSet, Described, Priced, Renamed, SeoSet, TaxCategorySet, VariantCompareAtSet,
};
use crate::error::CatalogError;
use crate::value_object::{
    TAX_CATEGORIES, TAX_CATEGORY_STANDARD, normalize_currency, normalize_description,
    normalize_name, normalize_seo, slugify,
};

use super::Price;

#[evento::command]
impl<E: Executor> super::Command<E> {
    /// Changes the display name and moves the product to the slug the new
    /// name gives, with a suffix when that one is taken; the old slug is free
    /// to any claim once the product has moved, and the release
    /// subscription frees its entry. A new name giving the slug of the old
    /// name (only the case or the accents differ) keeps the slug as it is.
    /// The same name is a no-op.
    pub async fn rename_product(
        &self,
        actor: &str,
        product_id: &str,
        name: &str,
        routing_key: Option<String>,
    ) -> Result<(), CatalogError> {
        let name = normalize_name(name)?;
        let product = self.require_editable(product_id).await?;
        if product.name == name {
            return Ok(());
        }
        let version = product.aggregate_version()?;
        let base = slugify(&name);
        let slug = if base == slugify(&product.name) {
            product.slug.clone()
        } else {
            self.claim_derived_slug(base, &product.id, version, routing_key.clone())
                .await?
        };
        let moved = slug != product.slug;
        let renamed = product
            .write()?
            .routing_key_opt(routing_key.clone())
            .event(&Renamed {
                name,
                slug: slug.clone(),
                renamed_by: actor.to_owned(),
            })
            .tenant(&self.tenant)
            .commit(&self.executor)
            .await;
        if let Err(err) = renamed {
            if moved
                && let Err(release) = self
                    .release_slug_claim(&slug, &product.id, version, routing_key)
                    .await
            {
                tracing::warn!(%slug, %release, "cannot give the slug back after a failed rename");
            }
            return Err(err.into());
        }
        Ok(())
    }

    /// Replaces the description; the same text is a no-op.
    pub async fn describe_product(
        &self,
        actor: &str,
        product_id: &str,
        description: &str,
        routing_key: Option<String>,
    ) -> Result<(), CatalogError> {
        let description = normalize_description(description)?;
        let product = self.require_editable(product_id).await?;
        if product.description == description {
            return Ok(());
        }
        product
            .write()?
            .routing_key_opt(routing_key)
            .event(&Described {
                description,
                described_by: actor.to_owned(),
            })
            .tenant(&self.tenant)
            .commit(&self.executor)
            .await?;
        Ok(())
    }

    /// Sets what search engines show for the product's page; empty goes back
    /// to its name and description. The same is a no-op.
    pub async fn set_product_seo(
        &self,
        actor: &str,
        product_id: &str,
        meta_title: &str,
        meta_description: &str,
        routing_key: Option<String>,
    ) -> Result<(), CatalogError> {
        let (meta_title, meta_description) = normalize_seo(meta_title, meta_description)?;
        let product = self.require_editable(product_id).await?;
        if product.meta_title == meta_title && product.meta_description == meta_description {
            return Ok(());
        }
        product
            .write()?
            .routing_key_opt(routing_key)
            .event(&SeoSet {
                meta_title,
                meta_description,
                set_by: actor.to_owned(),
            })
            .tenant(&self.tenant)
            .commit(&self.executor)
            .await?;
        Ok(())
    }

    /// Sets the price in minor units of a known currency. A product whose
    /// variants are priced in another currency is refused: a range across
    /// two of them would mean nothing. The same price is a no-op.
    pub async fn price_product(
        &self,
        actor: &str,
        product_id: &str,
        amount_cents: u64,
        currency: &str,
        routing_key: Option<String>,
    ) -> Result<(), CatalogError> {
        let currency = normalize_currency(currency)?;
        if amount_cents > crate::MAX_AMOUNT_CENTS {
            return Err(CatalogError::InvalidAmount);
        }
        let product = self.require_editable(product_id).await?;
        let price = Price {
            amount_cents,
            currency: currency.clone(),
        };
        if product.price.as_ref() == Some(&price) {
            return Ok(());
        }
        if product
            .variants
            .first()
            .is_some_and(|variant| variant.price.currency != currency)
        {
            return Err(CatalogError::CurrencyMismatch);
        }
        product
            .write()?
            .routing_key_opt(routing_key)
            .event(&Priced {
                amount_cents,
                currency,
                priced_by: actor.to_owned(),
            })
            .tenant(&self.tenant)
            .commit(&self.executor)
            .await?;
        Ok(())
    }

    /// Sets the price a reduction is announced against, struck through
    /// beside the price of the product (`variant_id` `None`) or of one of
    /// its variants; `None` announces none. It has to be above the price,
    /// and at most the lowest the thing was on sale at in the thirty days
    /// before its current price: a reduction is measured from a price
    /// shoppers really had, so a thing on sale at one price only is
    /// refused. The same is a no-op.
    pub async fn set_compare_at(
        &self,
        actor: &str,
        product_id: &str,
        variant_id: Option<&str>,
        amount_cents: Option<u64>,
        routing_key: Option<String>,
    ) -> Result<(), CatalogError> {
        let product = self.require_editable(product_id).await?;
        let set = match variant_id {
            Some(variant_id) => {
                product
                    .variant(variant_id)
                    .ok_or(CatalogError::VariantNotFound)?
                    .compare_at_cents
            }
            None => product.compare_at_cents,
        };
        if set == amount_cents {
            return Ok(());
        }
        if let Some(amount_cents) = amount_cents {
            if amount_cents > crate::MAX_AMOUNT_CENTS {
                return Err(CatalogError::InvalidAmount);
            }
            let price = product
                .amount_of(variant_id)
                .ok_or(CatalogError::NotPriced)?;
            if amount_cents <= price {
                return Err(CatalogError::CompareAtNotAbove);
            }
            let ceiling = product
                .compare_at_ceiling(variant_id)
                .ok_or(CatalogError::NoEarlierPrice)?;
            if amount_cents > ceiling {
                return Err(CatalogError::CompareAtAboveEarlier);
            }
        }
        let mut write = product.write()?;
        write.routing_key_opt(routing_key);
        match variant_id {
            Some(variant_id) => write.event(&VariantCompareAtSet {
                variant_id: variant_id.to_owned(),
                amount_cents,
                set_by: actor.to_owned(),
            }),
            None => write.event(&CompareAtSet {
                amount_cents,
                set_by: actor.to_owned(),
            }),
        };
        write.tenant(&self.tenant).commit(&self.executor).await?;
        Ok(())
    }

    /// Sets what kind of goods the product is for VAT, one of
    /// [`TAX_CATEGORIES`]; `standard` (or `None`) is the default. The same
    /// category is a no-op.
    pub async fn set_product_tax_category(
        &self,
        actor: &str,
        product_id: &str,
        category: Option<&str>,
        routing_key: Option<String>,
    ) -> Result<(), CatalogError> {
        let category = match category.map(str::trim) {
            None | Some("") | Some(TAX_CATEGORY_STANDARD) => None,
            Some(key) if TAX_CATEGORIES.iter().any(|(known, _)| *known == key) => {
                Some(key.to_owned())
            }
            Some(_) => return Err(CatalogError::InvalidTaxCategory),
        };
        let product = self.require_editable(product_id).await?;
        if product.tax_category == category {
            return Ok(());
        }
        product
            .write()?
            .routing_key_opt(routing_key)
            .event(&TaxCategorySet {
                category,
                set_by: actor.to_owned(),
            })
            .tenant(&self.tenant)
            .commit(&self.executor)
            .await?;
        Ok(())
    }
}
