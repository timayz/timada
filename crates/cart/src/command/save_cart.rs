use evento::{Executor, ProjectionAggregate};

use crate::{aggregator::CartSaved, error::CartError};

impl<E: Executor> super::Command<'_, E> {
    /// Keeps the cart under a name in "mes paniers sauvegardés".
    pub async fn save_cart(&self, id: impl Into<String>, name: String) -> Result<(), CartError> {
        let name = name.trim().to_owned();
        if name.is_empty() {
            return Err(CartError::Required("name"));
        }
        let cart = self.load_editable(id).await?;

        cart.write()?
            .event(&CartSaved { name })
            .commit(self.0)
            .await?;
        Ok(())
    }
}
