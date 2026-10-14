use evento::{Executor, ProjectionAggregate};

use crate::aggregator::{Described, Priced, Renamed};
use crate::error::CatalogError;
use crate::value_object::{normalize_currency, normalize_description, normalize_name};

use super::Price;

#[evento::command]
impl<E: Executor> super::Command<E> {
    /// Changes the display name; the same name is a no-op.
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
        product
            .write()?
            .routing_key_opt(routing_key)
            .event(&Renamed {
                name,
                renamed_by: actor.to_owned(),
            })
            .commit(&self.0)
            .await?;
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
            .commit(&self.0)
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
            .commit(&self.0)
            .await?;
        Ok(())
    }
}
