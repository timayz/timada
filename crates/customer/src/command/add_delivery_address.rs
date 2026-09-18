use evento::{Executor, ProjectionAggregate};
use timada_core::Address;

use crate::{
    aggregator::{DeliveryAddressAdded, PreferredDeliveryAddressChosen},
    error::CustomerError,
};

use super::address_id;

impl<E: Executor> super::Command<E> {
    /// Adds a delivery address; the first one becomes the preferred address
    /// in the same commit.
    pub async fn add_delivery_address(
        &self,
        id: impl Into<String>,
        address: Address,
    ) -> Result<String, CustomerError> {
        address.validate()?;
        let customer = self.load_existing(id).await?;
        let address_id = address_id(&customer.id, customer.next_address_seq);

        let mut write = customer.write()?;
        write.event(&DeliveryAddressAdded {
            address_id: address_id.clone(),
            address,
        });
        if customer.preferred.is_none() {
            write.event(&PreferredDeliveryAddressChosen {
                address_id: address_id.clone(),
            });
        }
        write.commit(&self.0).await?;
        Ok(address_id)
    }
}
