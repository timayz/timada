//! Owning a cart, and what its owner can do with a saved one.

use evento::{Executor, ProjectionAggregate};

use crate::{
    aggregator::{CartAssignedToCustomer, CartDiscarded, CartReopened},
    command::CartState,
    error::CartError,
    value_object::CartStatus,
};

impl<E: Executor> super::Command<'_, E> {
    /// Makes a signed-in customer the cart's owner. A no-op when they already
    /// are; a cart that belongs to someone else is refused.
    pub async fn assign_customer(
        &self,
        id: impl Into<String>,
        customer_id: &str,
    ) -> Result<(), CartError> {
        if customer_id.trim().is_empty() {
            return Err(CartError::Required("customer_id"));
        }
        let cart = self.load_editable(id).await?;
        match cart.customer_id.as_deref() {
            Some(owner) if owner == customer_id => return Ok(()),
            Some(_) => return Err(CartError::NotYourCart),
            None => {}
        }

        cart.write()?
            .event(&CartAssignedToCustomer {
                customer_id: customer_id.to_owned(),
            })
            .commit(self.0)
            .await?;
        Ok(())
    }

    /// A saved cart of `customer_id`; someone else's is reported as not found.
    async fn load_saved(
        &self,
        id: impl Into<String>,
        customer_id: &str,
    ) -> Result<CartState, CartError> {
        let cart = self.load(id).await?.ok_or(CartError::CartNotFound)?;
        if cart.customer_id.as_deref() != Some(customer_id) {
            return Err(CartError::CartNotFound);
        }
        Ok(cart)
    }

    /// Makes a saved cart the current one again. A no-op when it is already open.
    pub async fn reopen_cart(
        &self,
        id: impl Into<String>,
        customer_id: &str,
    ) -> Result<(), CartError> {
        let cart = self.load_saved(id, customer_id).await?;
        match cart.status {
            CartStatus::Open => return Ok(()),
            CartStatus::Saved => {}
            CartStatus::CheckedOut => return Err(CartError::CartAlreadyCheckedOut),
            CartStatus::Discarded => return Err(CartError::CartDiscarded),
        }

        cart.write()?.event(&CartReopened).commit(self.0).await?;
        Ok(())
    }

    /// Deletes a saved cart. A no-op when already deleted; an open cart is
    /// not something to delete — it is simply abandoned.
    pub async fn discard_cart(
        &self,
        id: impl Into<String>,
        customer_id: &str,
    ) -> Result<(), CartError> {
        let cart = self.load_saved(id, customer_id).await?;
        match cart.status {
            CartStatus::Discarded => return Ok(()),
            CartStatus::Saved => {}
            CartStatus::Open => return Err(CartError::NotSaved),
            CartStatus::CheckedOut => return Err(CartError::CartAlreadyCheckedOut),
        }

        cart.write()?.event(&CartDiscarded).commit(self.0).await?;
        Ok(())
    }
}
