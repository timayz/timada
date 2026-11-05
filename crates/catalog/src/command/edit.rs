use evento::{Executor, ProjectionAggregate};

use crate::aggregator::{Described, Priced, Renamed, TaxCategorySet};
use crate::error::CatalogError;
use crate::value_object::{
    TAX_CATEGORIES, TAX_CATEGORY_STANDARD, normalize_currency, normalize_description,
    normalize_name, slugify,
};

use super::Price;

#[evento::command]
impl<E: Executor> super::Command<E> {
    /// Changes the display name and moves the product to the slug the new
    /// name gives, with a suffix when that one is taken; the old slug is freed
    /// once the product has moved. A new name giving the slug of the old name
    /// (only the case or the accents differ) keeps the slug as it is. The same
    /// name is a no-op.
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
        let base = slugify(&name);
        let slug = if base == slugify(&product.name) {
            product.slug.clone()
        } else {
            self.claim_derived_slug(base, &product.id, routing_key.clone())
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
            if moved && let Err(release) = self.release_slug(&slug, &product.id, routing_key).await
            {
                tracing::warn!(%slug, %release, "cannot give the slug back after a failed rename");
            }
            return Err(err.into());
        }
        if moved
            && let Err(release) = self
                .release_slug(&product.slug, &product.id, routing_key)
                .await
        {
            tracing::warn!(slug = %product.slug, %release, "cannot give the old slug back after a rename");
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
