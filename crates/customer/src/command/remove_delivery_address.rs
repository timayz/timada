use evento::{Executor, ProjectionAggregate};

use crate::{aggregator::DeliveryAddressRemoved, error::CustomerError};

impl<E: Executor> super::Command<'_, E> {
    /// Removes a delivery address. The preferred one can only go once it is
    /// the last address left.
    pub async fn remove_delivery_address(
        &self,
        id: impl Into<String>,
        address_id: String,
    ) -> Result<(), CustomerError> {
        let customer = self.load_existing(id).await?;
        if !customer.has_delivery_address(&address_id) {
            return Err(CustomerError::AddressNotFound);
        }
        let is_preferred = customer.preferred.as_deref() == Some(address_id.as_str());
        if is_preferred && customer.delivery_ids.len() > 1 {
            return Err(CustomerError::CannotRemovePreferred);
        }

        customer
            .write()?
            .event(&DeliveryAddressRemoved { address_id })
            .commit(self.0)
            .await?;
        Ok(())
    }
}
