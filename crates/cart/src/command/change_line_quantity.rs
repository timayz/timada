use evento::{Executor, ProjectionAggregate};

use crate::{aggregator::CartLineQuantityChanged, error::CartError};

impl<E: Executor> super::Command<E> {
    pub async fn change_line_quantity(
        &self,
        id: impl Into<String>,
        product_id: String,
        quantity: u32,
    ) -> Result<(), CartError> {
        if quantity == 0 {
            return Err(CartError::InvalidQuantity);
        }
        let cart = self.load_editable(id).await?;
        if !cart.products.contains(&product_id) {
            return Err(CartError::LineNotFound(product_id));
        }

        cart.write()?
            .event(&CartLineQuantityChanged {
                product_id,
                quantity,
            })
            .commit(&self.0)
            .await?;
        Ok(())
    }
}
