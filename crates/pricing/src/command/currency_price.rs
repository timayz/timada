use evento::{Executor, ProjectionAggregate};
use timada_core::{Money, currency::is_currency_code};

use crate::{
    aggregator::{CurrencyPriceRemoved, CurrencyPriceSet, ProductPriceChanged},
    error::PricingError,
};

impl<E: Executor> super::Command<'_, E> {
    /// Gives the product its price in `price_incl_tax`'s currency: the listed
    /// price when that is the currency it was listed in, a price next to it
    /// otherwise. Setting the price it already has writes nothing.
    pub async fn set_currency_price(
        &self,
        id: impl Into<String>,
        price_incl_tax: Money,
    ) -> Result<(), PricingError> {
        if !price_incl_tax.is_positive() {
            return Err(PricingError::InvalidAmount);
        }
        if !is_currency_code(&price_incl_tax.currency) {
            return Err(PricingError::InvalidCurrency(price_incl_tax.currency));
        }
        let price = self.load_active(id).await?;
        if price.price_in(&price_incl_tax.currency) == Some(price_incl_tax.minor) {
            return Ok(());
        }
        if price.currency == price_incl_tax.currency {
            price
                .write()?
                .event(&ProductPriceChanged { price_incl_tax })
                .commit(self.0)
                .await?;
        } else {
            price
                .write()?
                .event(&CurrencyPriceSet { price_incl_tax })
                .commit(self.0)
                .await?;
        }
        Ok(())
    }

    /// Stops selling the product in `currency`. A no-op when it has no price
    /// there; the currency it was listed in cannot be removed — withdraw the
    /// price to stop selling the product altogether.
    pub async fn remove_currency_price(
        &self,
        id: impl Into<String>,
        currency: &str,
    ) -> Result<(), PricingError> {
        let price = self.load_active(id).await?;
        if price.currency == currency {
            return Err(PricingError::ListedCurrency(currency.to_owned()));
        }
        if price.price_in(currency).is_none() {
            return Ok(());
        }
        price
            .write()?
            .event(&CurrencyPriceRemoved {
                currency: currency.to_owned(),
            })
            .commit(self.0)
            .await?;
        Ok(())
    }
}
