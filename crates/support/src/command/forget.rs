use evento::{Executor, ProjectionAggregate, WriteError};
use timada_core::Keys;

use crate::aggregator::Forgotten;
use crate::error::SupportError;

#[evento::command]
impl<E: Executor> super::Command<E> {
    /// Erases a message, as its sender asked: its key is destroyed, which
    /// leaves what it holds unreadable, and the read model drops its row.
    /// Already erased is a no-op; unknown is [`SupportError::NotFound`].
    pub async fn forget_message(
        &self,
        keys: &dyn Keys,
        actor: &str,
        message_id: &str,
        routing_key: Option<String>,
    ) -> Result<(), SupportError> {
        loop {
            let message = self
                .load_message_any(message_id)
                .await?
                .ok_or(SupportError::NotFound)?;
            if message.forgotten {
                return Ok(());
            }
            match message
                .write()?
                .routing_key_opt(routing_key.clone())
                .event(&Forgotten {
                    key_id: message.key_id.clone(),
                    forgotten_by: actor.to_owned(),
                })
                .tenant(&self.tenant)
                .commit(&self.executor)
                .await
            {
                Ok(_) => {
                    // Whoever keeps the keys also destroys it on reading the
                    // event, so a failure here is only a delay.
                    let _ = keys.delete(&message.key_id).await;
                    return Ok(());
                }
                // The message changed meanwhile (handled, erased by another
                // member): decide again on the fresh state.
                Err(WriteError::InvalidOriginalVersion) => continue,
                Err(err) => return Err(err.into()),
            }
        }
    }
}
