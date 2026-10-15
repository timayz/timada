use evento::{Executor, ProjectionAggregate};
use timada_orders::{ShippingAddress, normalize_shipping};

use crate::aggregator::{AddressRemoved, AddressSaved};
use crate::error::ShopperError;
use crate::value_object::{MAX_ADDRESSES, address_id};

#[evento::command]
impl<E: Executor> super::Command<E> {
    /// Keeps an address to ship to again and returns its id. Saving one the
    /// shopper already has moves it first; an eleventh is refused.
    pub async fn save_address(
        &self,
        shopper_id: &str,
        address: ShippingAddress,
        routing_key: Option<String>,
    ) -> Result<String, ShopperError> {
        let address = normalize_shipping(address).map_err(ShopperError::InvalidAddress)?;
        let shopper = self
            .load_shopper(shopper_id)
            .await?
            .ok_or(ShopperError::NotFound)?;
        let id = address_id(&shopper.id, &address);
        let known = shopper.address(&id).is_some();
        if known
            && shopper
                .addresses
                .first()
                .is_some_and(|first| first.id == id)
        {
            return Ok(id);
        }
        if !known && shopper.addresses.len() >= MAX_ADDRESSES {
            return Err(ShopperError::TooManyAddresses);
        }
        shopper
            .write()?
            .routing_key_opt(routing_key)
            .event(&AddressSaved {
                address_id: id.clone(),
                address,
            })
            .commit(&self.0)
            .await?;
        Ok(id)
    }

    /// Forgets a saved address; one already gone is a no-op.
    pub async fn remove_address(
        &self,
        shopper_id: &str,
        address_id: &str,
        routing_key: Option<String>,
    ) -> Result<(), ShopperError> {
        let shopper = self
            .load_shopper(shopper_id)
            .await?
            .ok_or(ShopperError::NotFound)?;
        if shopper.address(address_id).is_none() {
            return Ok(());
        }
        shopper
            .write()?
            .routing_key_opt(routing_key)
            .event(&AddressRemoved {
                address_id: address_id.to_owned(),
            })
            .commit(&self.0)
            .await?;
        Ok(())
    }
}
