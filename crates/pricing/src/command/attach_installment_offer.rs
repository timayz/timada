use evento::{Executor, ProjectionAggregate};
use timada_core::Money;

use crate::{
    aggregator::InstallmentOfferAttached, error::PricingError, value_object::InstallmentOffer,
};

impl<E: Executor> super::Command<E> {
    pub async fn attach_installment_offer(
        &self,
        id: impl Into<String>,
        offer: InstallmentOffer,
    ) -> Result<(), PricingError> {
        if !(2..=4).contains(&offer.count) {
            return Err(PricingError::InvalidInstallmentCount(offer.count));
        }
        if offer.fee.is_negative() {
            return Err(PricingError::NegativeAmount);
        }
        let price = self.load_active(id).await?;
        Money::zero(&price.currency).same_currency(&offer.fee)?;

        price
            .write()?
            .event(&InstallmentOfferAttached { offer })
            .commit(&self.0)
            .await?;
        Ok(())
    }
}
