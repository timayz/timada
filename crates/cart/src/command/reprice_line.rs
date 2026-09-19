use evento::{Executor, ProjectionAggregate};
use timada_core::Money;

use crate::{aggregator::CartLineRepriced, error::CartError};

impl<E: Executor> super::Command<'_, E> {
    /// Brings a line to the product's current price. Returns whether the
    /// price changed: a line already at that price is left alone, so a
    /// storefront can call this for every line of a cart it is about to show.
    pub async fn reprice_line(
        &self,
        id: impl Into<String>,
        product_id: &str,
        unit_price: Money,
    ) -> Result<bool, CartError> {
        if !unit_price.is_positive() {
            return Err(CartError::Required("unit_price"));
        }
        let cart = self.load_editable(id).await?;
        let Some((_, current)) = cart.prices.iter().find(|(p, _)| p == product_id) else {
            return Err(CartError::LineNotFound(product_id.to_owned()));
        };
        current.same_currency(&unit_price)?;
        if *current == unit_price {
            return Ok(false);
        }

        cart.write()?
            .event(&CartLineRepriced {
                product_id: product_id.to_owned(),
                unit_price,
            })
            .commit(self.0)
            .await?;
        Ok(true)
    }
}
