use evento::{Executor, ProjectionAggregate};

use crate::{aggregator::PromoCodeApplied, error::CartError};

impl<E: Executor> super::Command<E> {
    /// Records the code typed in the "code promo ou bon d'achat" box. It is
    /// advisory here: the promotion context validates it when the order is placed.
    pub async fn apply_promo_code(
        &self,
        id: impl Into<String>,
        code: String,
    ) -> Result<(), CartError> {
        let code = code.trim().to_uppercase();
        if code.is_empty() {
            return Err(CartError::Required("code"));
        }
        let cart = self.load_editable(id).await?;

        cart.write()?
            .event(&PromoCodeApplied { code })
            .commit(&self.0)
            .await?;
        Ok(())
    }
}
