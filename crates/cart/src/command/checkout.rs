use evento::{Executor, ProjectionAggregate};
use timada_core::Address;

use crate::{
    aggregator::CartCheckedOut,
    error::CartError,
    value_object::{DeliveryChoice, PaymentMode},
};

#[derive(Debug, Clone)]
pub struct Checkout {
    /// Overrides the customer the cart was opened for (guest carts get one here).
    pub customer_id: Option<String>,
    pub delivery_address: Address,
    pub billing_address: Address,
    pub delivery: DeliveryChoice,
    pub payment_mode: PaymentMode,
}

impl<E: Executor> super::Command<E> {
    /// "Passer commande": freezes the cart and publishes the checkout fact the
    /// order context turns into an order.
    pub async fn checkout(&self, id: impl Into<String>, cmd: Checkout) -> Result<(), CartError> {
        let cart = self.load_editable(id).await?;
        if cart.products.is_empty() {
            return Err(CartError::EmptyCart);
        }
        let Some(customer_id) = cmd.customer_id.or(cart.customer_id.clone()) else {
            return Err(CartError::CustomerRequired);
        };
        cmd.delivery_address.validate()?;
        cmd.billing_address.validate()?;
        if cmd.delivery.method_code.trim().is_empty() {
            return Err(CartError::Required("delivery.method_code"));
        }
        if let PaymentMode::Installments { count } = cmd.payment_mode
            && !(2..=4).contains(&count)
        {
            return Err(CartError::InvalidPaymentMode);
        }

        cart.write()?
            .event(&CartCheckedOut {
                customer_id: customer_id.clone(),
                delivery_address: cmd.delivery_address,
                billing_address: cmd.billing_address,
                delivery: cmd.delivery,
                payment_mode: cmd.payment_mode,
            })
            .commit(&self.0)
            .await?;
        tracing::info!(cart_id = %cart.id, %customer_id, "cart checked out");
        Ok(())
    }
}
