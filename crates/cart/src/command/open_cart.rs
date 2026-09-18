use evento::Executor;

use crate::{aggregator::CartOpened, error::CartError};

#[evento::command]
impl<E: Executor> super::Command<'_, E> {
    /// Starts a new cart, tied to a customer when one is signed in.
    pub async fn open_cart(
        &self,
        customer_id: Option<String>,
        routing_key: Option<String>,
    ) -> Result<String, CartError> {
        let id = evento::create()
            .routing_key_opt(routing_key)
            .event(&CartOpened { customer_id })
            .commit(self.0)
            .await?;
        tracing::info!(cart_id = %id, "cart opened");
        Ok(id)
    }
}
