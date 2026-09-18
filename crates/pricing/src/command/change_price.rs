use evento::{Executor, ProjectionAggregate};
use timada_core::Money;

use crate::{aggregator::ProductPriceChanged, error::PricingError};

impl<E: Executor> super::Command<'_, E> {
    pub async fn change_price(
        &self,
        id: impl Into<String>,
        price_incl_tax: Money,
    ) -> Result<(), PricingError> {
        if !price_incl_tax.is_positive() {
            return Err(PricingError::InvalidAmount);
        }
        let price = self.load_active(id).await?;
        Money::zero(&price.currency).same_currency(&price_incl_tax)?;

        price
            .write()?
            .event(&ProductPriceChanged { price_incl_tax })
            .commit(self.0)
            .await?;
        Ok(())
    }
}
