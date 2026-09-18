use evento::{Executor, ProjectionAggregate};
use timada_core::Address;

use crate::{aggregator::DeliveryAddressChanged, error::CustomerError};

impl<E: Executor> super::Command<E> {
    pub async fn change_delivery_address(
        &self,
        id: impl Into<String>,
        address_id: String,
        address: Address,
    ) -> Result<(), CustomerError> {
        address.validate()?;
        let customer = self.load_existing(id).await?;
        if !customer.has_delivery_address(&address_id) {
            return Err(CustomerError::AddressNotFound);
        }

        customer
            .write()?
            .event(&DeliveryAddressChanged {
                address_id,
                address,
            })
            .commit(&self.0)
            .await?;
        Ok(())
    }
}
