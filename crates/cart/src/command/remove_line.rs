use evento::{Executor, ProjectionAggregate};

use crate::{aggregator::CartLineRemoved, error::CartError};

impl<E: Executor> super::Command<E> {
    pub async fn remove_line(
        &self,
        id: impl Into<String>,
        product_id: String,
    ) -> Result<(), CartError> {
        let cart = self.load_editable(id).await?;
        if !cart.products.contains(&product_id) {
            return Err(CartError::LineNotFound(product_id));
        }

        cart.write()?
            .event(&CartLineRemoved { product_id })
            .commit(&self.0)
            .await?;
        Ok(())
    }
}
