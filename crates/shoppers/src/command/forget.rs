use evento::{Executor, ProjectionAggregate, WriteError};

use crate::aggregator::Forgotten;
use crate::error::ShopperError;

#[evento::command]
impl<E: Executor> super::Command<E> {
    /// Deletes the shopper's account: the addresses are forgotten and every
    /// session is void. The orders placed with the address stay with the
    /// shop. Unknown or already forgotten is a no-op.
    pub async fn forget(
        &self,
        shopper_id: &str,
        routing_key: Option<String>,
    ) -> Result<(), ShopperError> {
        loop {
            let Some(shopper) = self.load_shopper(shopper_id).await? else {
                return Ok(());
            };
            match shopper
                .write()?
                .routing_key_opt(routing_key.clone())
                .event(&Forgotten)
                .commit(&self.0)
                .await
            {
                Ok(_) => return Ok(()),
                // The shopper changed meanwhile (an address saved, another
                // deletion): decide again on the fresh state.
                Err(WriteError::InvalidOriginalVersion) => continue,
                Err(err) => return Err(err.into()),
            }
        }
    }
}
