use evento::{Executor, ProjectionAggregate};
use timada_core::Money;

use crate::{aggregator::CartLineAdded, error::CartError};

#[derive(Debug, Clone)]
pub struct AddLine {
    pub product_id: String,
    pub name: String,
    pub quantity: u32,
    pub unit_price: Money,
    pub warranty_months: u16,
}

impl<E: Executor> super::Command<'_, E> {
    /// Puts a product in the cart with the price seen at that moment. A cart
    /// is locked to the currency of its first line.
    pub async fn add_line(&self, id: impl Into<String>, cmd: AddLine) -> Result<(), CartError> {
        if cmd.quantity == 0 {
            return Err(CartError::InvalidQuantity);
        }
        let cart = self.load_editable(id).await?;
        if cart.products.contains(&cmd.product_id) {
            return Err(CartError::LineAlreadyInCart(cmd.product_id));
        }
        if let Some(currency) = &cart.currency {
            Money::zero(currency).same_currency(&cmd.unit_price)?;
        }

        cart.write()?
            .event(&CartLineAdded {
                product_id: cmd.product_id,
                name: cmd.name,
                quantity: cmd.quantity,
                unit_price: cmd.unit_price,
                warranty_months: cmd.warranty_months,
            })
            .commit(self.0)
            .await?;
        Ok(())
    }
}
