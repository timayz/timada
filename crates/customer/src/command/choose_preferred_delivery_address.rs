use evento::{Executor, ProjectionAggregate};

use crate::{aggregator::PreferredDeliveryAddressChosen, error::CustomerError};

impl<E: Executor> super::Command<E> {
    pub async fn choose_preferred_delivery_address(
        &self,
        id: impl Into<String>,
        address_id: String,
    ) -> Result<(), CustomerError> {
        let customer = self.load_existing(id).await?;
        if !customer.has_delivery_address(&address_id) {
            return Err(CustomerError::AddressNotFound);
        }
        if customer.preferred.as_deref() == Some(address_id.as_str()) {
            return Ok(());
        }

        customer
            .write()?
            .event(&PreferredDeliveryAddressChosen { address_id })
            .commit(&self.0)
            .await?;
        Ok(())
    }
}
