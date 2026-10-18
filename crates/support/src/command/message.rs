use evento::{Executor, ProjectionAggregate};
use timada_core::keys::{self, Key, Keys, new_key_id};

use crate::aggregator::{Handled, Received, Reopened};
use crate::error::SupportError;
use crate::value_object::{MessageStatus, check_body, check_name, new_message_id, normalize_email};

use super::{BODY, EMAIL, FIRST_NAME, LAST_NAME, Message, field_aad};

/// What a visitor sent through the contact form.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReceiveMessage {
    pub first_name: String,
    pub last_name: String,
    pub email: String,
    pub body: String,
    /// A language tag or a whole `Accept-Language` value; only the primary
    /// subtag is kept.
    pub language: Option<String>,
}

/// A [`ReceiveMessage`] as the store records it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Checked {
    first_name: String,
    last_name: String,
    email: String,
    body: String,
    language: Option<String>,
}

impl ReceiveMessage {
    /// The rules [`Command::receive_message`] applies, for a form to refuse
    /// before anything else.
    pub fn validate(&self) -> Result<(), SupportError> {
        self.check().map(|_| ())
    }

    fn check(&self) -> Result<Checked, SupportError> {
        Ok(Checked {
            first_name: check_name(&self.first_name)?,
            last_name: check_name(&self.last_name)?,
            email: normalize_email(&self.email)?,
            body: check_body(&self.body)?,
            language: self
                .language
                .as_deref()
                .and_then(timada_core::primary_language),
        })
    }
}

#[evento::command]
impl<E: Executor> super::Command<E> {
    /// Records a message waiting for the merchant, under a fresh id, sealed
    /// under a fresh key that `keys` holds before the event names it. A key
    /// that cannot be kept (no majority of nodes reached) refuses the
    /// message; a commit that fails lets the key go.
    pub async fn receive_message(
        &self,
        keys: &dyn Keys,
        actor: &str,
        cmd: ReceiveMessage,
        routing_key: Option<String>,
    ) -> Result<Message, SupportError> {
        let checked = cmd.check()?;
        let id = new_message_id();
        let key_id = new_key_id();
        let key = Key::generate();
        let seal = |field: &str, plain: &str| {
            keys::seal(&key, &field_aad(&id, &key_id, field), plain.as_bytes())
        };
        let event = Received {
            first_name: seal(FIRST_NAME, &checked.first_name)?,
            last_name: seal(LAST_NAME, &checked.last_name)?,
            email: seal(EMAIL, &checked.email)?,
            body: seal(BODY, &checked.body)?,
            key_id: key_id.clone(),
            language: checked.language,
            received_by: actor.to_owned(),
        };
        keys.put(&key_id, &key).await?;
        if let Err(err) = evento::append(&id)
            .original_version(0)
            .routing_key_opt(routing_key)
            .event(&event)
            .commit(&self.0)
            .await
        {
            let _ = keys.delete(&key_id).await;
            return Err(err.into());
        }
        let message = self
            .load_message(&id)
            .await?
            .ok_or(SupportError::NotFound)?;
        Ok(message.open(&key)?)
    }

    /// Marks it answered; already handled is a no-op.
    pub async fn handle_message(
        &self,
        actor: &str,
        message_id: &str,
        routing_key: Option<String>,
    ) -> Result<(), SupportError> {
        let message = self
            .load_message(message_id)
            .await?
            .ok_or(SupportError::NotFound)?;
        if message.status == MessageStatus::Handled {
            return Ok(());
        }
        message
            .write()?
            .routing_key_opt(routing_key)
            .event(&Handled {
                handled_by: actor.to_owned(),
            })
            .commit(&self.0)
            .await?;
        Ok(())
    }

    /// Puts it back among the waiting ones; already open is a no-op.
    pub async fn reopen_message(
        &self,
        actor: &str,
        message_id: &str,
        routing_key: Option<String>,
    ) -> Result<(), SupportError> {
        let message = self
            .load_message(message_id)
            .await?
            .ok_or(SupportError::NotFound)?;
        if message.status == MessageStatus::Open {
            return Ok(());
        }
        message
            .write()?
            .routing_key_opt(routing_key)
            .event(&Reopened {
                reopened_by: actor.to_owned(),
            })
            .commit(&self.0)
            .await?;
        Ok(())
    }
}
