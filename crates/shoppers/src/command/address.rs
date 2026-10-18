use evento::{Executor, ProjectionAggregate};
use timada_orders::{ShippingAddress, normalize_shipping};

use crate::aggregator::{AddressRemoved, AddressSaved};
use crate::error::ShopperError;
use crate::value_object::{MAX_ADDRESSES, address_id};
use timada_core::keys::{self as key, Keys as ShopperKeys};

use super::address_aad;

#[evento::command]
impl<E: Executor> super::Command<E> {
    /// Keeps an address to ship to again, sealed under the shopper's key,
    /// and returns its id. Saving one the
    /// shopper already has moves it first; an eleventh is refused.
    pub async fn save_address(
        &self,
        keys: &dyn ShopperKeys,
        shopper_id: &str,
        address: ShippingAddress,
        routing_key: Option<String>,
    ) -> Result<String, ShopperError> {
        let address = normalize_shipping(address).map_err(ShopperError::InvalidAddress)?;
        let shopper = self
            .load_shopper(shopper_id)
            .await?
            .ok_or(ShopperError::NotFound)?;
        // A destroyed key is an account deleted meanwhile.
        let key = keys
            .get(&shopper.key_id)
            .await?
            .ok_or(ShopperError::NotFound)?;
        let id = address_id(&key, &address);
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
                address: key::seal(
                    &key,
                    &address_aad(&shopper.id, &shopper.key_id, &id),
                    &bitcode::encode(&address),
                )?,
                address_id: id.clone(),
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
