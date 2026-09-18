use evento::{Executor, ProjectionAggregate};
use timada_core::Money;

use crate::{aggregator::EcoParticipationChanged, error::PricingError};

impl<E: Executor> super::Command<E> {
    pub async fn change_eco_participation(
        &self,
        id: impl Into<String>,
        eco_participation: Money,
    ) -> Result<(), PricingError> {
        if eco_participation.is_negative() {
            return Err(PricingError::NegativeAmount);
        }
        let price = self.load_active(id).await?;
        Money::zero(&price.currency).same_currency(&eco_participation)?;

        price
            .write()?
            .event(&EcoParticipationChanged { eco_participation })
            .commit(&self.0)
            .await?;
        Ok(())
    }
}
