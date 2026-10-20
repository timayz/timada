use evento::{Executor, ProjectionAggregate, WriteError};
use timada_core::Keys;

use crate::aggregator::Forgotten;
use crate::error::NewsletterError;

#[evento::command]
impl<E: Executor> super::Command<E> {
    /// Erases a subscriber, as they or the merchant asked: their key is
    /// destroyed, which leaves the address unreadable and every link mailed
    /// to them dead, and the read model drops their row. Already erased is a
    /// no-op; unknown is [`NewsletterError::NotFound`].
    pub async fn forget_subscriber(
        &self,
        keys: &dyn Keys,
        actor: &str,
        subscriber_id: &str,
        routing_key: Option<String>,
    ) -> Result<(), NewsletterError> {
        loop {
            let state = self
                .load_subscriber_any(subscriber_id)
                .await?
                .ok_or(NewsletterError::NotFound)?;
            if state.forgotten {
                return Ok(());
            }
            match state
                .write()?
                .routing_key_opt(routing_key.clone())
                .event(&Forgotten {
                    key_id: state.key_id.clone(),
                    forgotten_by: actor.to_owned(),
                })
                .commit(&self.0)
                .await
            {
                Ok(_) => {
                    // Whoever keeps the keys also destroys it on reading the
                    // event, so a failure here is only a delay.
                    let _ = keys.delete(&state.key_id).await;
                    return Ok(());
                }
                // The subscriber changed meanwhile: decide again.
                Err(WriteError::InvalidOriginalVersion) => continue,
                Err(err) => return Err(err.into()),
            }
        }
    }
}
