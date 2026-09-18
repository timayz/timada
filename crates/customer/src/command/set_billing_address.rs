use evento::{Executor, ProjectionAggregate};
use timada_core::Address;

use crate::{aggregator::BillingAddressSet, error::CustomerError};

impl<E: Executor> super::Command<E> {
    pub async fn set_billing_address(
        &self,
        id: impl Into<String>,
        address: Address,
    ) -> Result<(), CustomerError> {
        address.validate()?;
        let customer = self.load_existing(id).await?;

        customer
            .write()?
            .event(&BillingAddressSet { address })
            .commit(&self.0)
            .await?;
        Ok(())
    }
}
