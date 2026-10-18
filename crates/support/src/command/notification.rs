use evento::{Executor, WriteError};

use crate::aggregator::Notified;
use crate::error::SupportError;
use crate::value_object::message_notification_id;

#[evento::command]
impl<E: Executor> super::Command<E> {
    /// Takes the telling of a message's arrival, before the notice goes out.
    /// Returns whether this caller took it: `false` when it was told already,
    /// by another node say.
    pub async fn claim_notification(
        &self,
        message_id: &str,
        actor: &str,
        routing_key: Option<String>,
    ) -> Result<bool, SupportError> {
        let claimed = evento::append(message_notification_id(message_id))
            .original_version(0)
            .routing_key_opt(routing_key)
            .event(&Notified {
                message_id: message_id.to_owned(),
                mailed_by: actor.to_owned(),
            })
            .commit(&self.0)
            .await;
        match claimed {
            Ok(_) => Ok(true),
            Err(WriteError::InvalidOriginalVersion) => Ok(false),
            Err(err) => Err(err.into()),
        }
    }
}
