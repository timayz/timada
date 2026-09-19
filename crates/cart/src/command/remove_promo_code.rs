use evento::{Executor, ProjectionAggregate};

use crate::{aggregator::PromoCodeRemoved, error::CartError};

impl<E: Executor> super::Command<'_, E> {
    /// Takes the code off the cart. A cart without a code is left as it is,
    /// so a double submit is harmless.
    pub async fn remove_promo_code(&self, id: impl Into<String>) -> Result<(), CartError> {
        let cart = self.load_editable(id).await?;
        if !cart.has_promo_code {
            return Ok(());
        }

        cart.write()?
            .event(&PromoCodeRemoved)
            .commit(self.0)
            .await?;
        Ok(())
    }
}
