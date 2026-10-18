use evento::{Executor, ProjectionAggregate};

use crate::aggregator::{Handled, Received, Reopened};
use crate::error::SupportError;
use crate::value_object::{MessageStatus, check_body, check_name, new_message_id, normalize_email};

use super::MessageState;

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
    /// Records a message waiting for the merchant, under a fresh id.
    pub async fn receive_message(
        &self,
        actor: &str,
        cmd: ReceiveMessage,
        routing_key: Option<String>,
    ) -> Result<MessageState, SupportError> {
        let checked = cmd.check()?;
        let id = new_message_id();
        evento::append(&id)
            .original_version(0)
            .routing_key_opt(routing_key)
            .event(&Received {
                first_name: checked.first_name,
                last_name: checked.last_name,
                email: checked.email,
                body: checked.body,
                language: checked.language,
                received_by: actor.to_owned(),
            })
            .commit(&self.0)
            .await?;
        self.load_message(&id).await?.ok_or(SupportError::NotFound)
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
