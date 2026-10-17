use evento::{Executor, ProjectionAggregate, WriteError};

use crate::aggregator::Forgotten;
use crate::error::ShopperError;
use crate::key::ShopperKeys;

#[evento::command]
impl<E: Executor> super::Command<E> {
    /// Deletes the shopper's account: their key is destroyed, which leaves
    /// what it sealed unreadable, and every session is void. The orders
    /// placed with the address stay with the shop. Unknown or already
    /// forgotten is a no-op.
    pub async fn forget(
        &self,
        keys: &dyn ShopperKeys,
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
                .event(&Forgotten {
                    key_id: shopper.key_id.clone(),
                })
                .commit(&self.0)
                .await
            {
                Ok(_) => {
                    // Whoever keeps the keys also destroys it on reading the
                    // event, so a failure here is only a delay.
                    let _ = keys.delete(&shopper.key_id).await;
                    return Ok(());
                }
                // The shopper changed meanwhile (an address saved, another
                // deletion): decide again on the fresh state.
                Err(WriteError::InvalidOriginalVersion) => continue,
                Err(err) => return Err(err.into()),
            }
        }
    }
}
