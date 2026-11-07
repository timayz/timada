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
        self.forget(keys, actor, subscriber_id, None, routing_key)
            .await
    }

    /// Erases the subscription sealed under `key_id`, as the deletion of a
    /// shopper account names it. The address may have subscribed again
    /// since, on the same stream under a new key: that newer subscription
    /// is left alone. Already erased, or unknown, is a no-op.
    pub async fn forget_subscription(
        &self,
        keys: &dyn Keys,
        actor: &str,
        subscriber_id: &str,
        key_id: &str,
        routing_key: Option<String>,
    ) -> Result<(), NewsletterError> {
        match self
            .forget(keys, actor, subscriber_id, Some(key_id), routing_key)
            .await
        {
            Err(NewsletterError::NotFound) => Ok(()),
            done => done,
        }
    }
}

impl<E: Executor> super::Command<E> {
    /// Erases the subscriber, when `key_id` is `None` or the key it is
    /// sealed under now.
    async fn forget(
        &self,
        keys: &dyn Keys,
        actor: &str,
        subscriber_id: &str,
        key_id: Option<&str>,
        routing_key: Option<String>,
    ) -> Result<(), NewsletterError> {
        loop {
            let state = self
                .load_subscriber_any(subscriber_id)
                .await?
                .ok_or(NewsletterError::NotFound)?;
            if state.forgotten || key_id.is_some_and(|key_id| key_id != state.key_id) {
                return Ok(());
            }
            match state
                .write()?
                .routing_key_opt(routing_key.clone())
                .event(&Forgotten {
                    key_id: state.key_id.clone(),
                    forgotten_by: actor.to_owned(),
                })
                .tenant(&self.tenant)
                .commit(&self.executor)
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
