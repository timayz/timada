//! Write side: the command gateway and the strict write model it loads.
//!
//! The write model folds *all* events of a message (`strict()`), so the
//! version it observes, and `write()` sends as `original_version`, never
//! goes stale. It is not snapshotted: a message is a handful of events.

mod message;

use std::ops::Deref;

pub use message::*;

use evento::{Executor, Projection, metadata::Event};

use crate::aggregator::{Handled, Received, Reopened};
use crate::value_object::MessageStatus;

/// The command gateway. Cheap to clone when `E` is.
pub struct Command<E: Executor>(pub E);

impl<E: Executor> Deref for Command<E> {
    type Target = E;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[evento::projection(id = id, name = "timada-support/MessageState")]
#[evento::snapshot(none)]
#[derive(Debug, PartialEq, Eq)]
pub struct MessageState {
    pub id: String,
    pub first_name: String,
    pub last_name: String,
    pub email: String,
    pub body: String,
    pub language: Option<String>,
    pub status: MessageStatus,
    /// Unix seconds of `Received`.
    pub received_at: u64,
    /// Unix seconds of `Handled`, while it is.
    pub handled_at: Option<u64>,
    /// Unix seconds of the last event.
    pub updated_at: u64,
}

impl MessageState {
    pub fn is_handled(&self) -> bool {
        self.status == MessageStatus::Handled
    }

    /// `Ada Lovelace`.
    pub fn full_name(&self) -> String {
        format!("{} {}", self.first_name, self.last_name)
    }
}

pub fn message_projection<E: Executor>() -> Projection<E, MessageState> {
    Projection::new::<crate::aggregator::Message>()
        .handler(on_received())
        .handler(on_handled())
        .handler(on_reopened())
        .strict()
}

#[evento::handler]
async fn on_received(event: Event<Received>, message: &mut MessageState) -> anyhow::Result<()> {
    let data = &event.data;
    message.id = event.aggregate_id.to_owned();
    message.first_name = data.first_name.to_owned();
    message.last_name = data.last_name.to_owned();
    message.email = data.email.to_owned();
    message.body = data.body.to_owned();
    message.language = data.language.to_owned();
    message.status = MessageStatus::Open;
    message.received_at = event.timestamp;
    message.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_handled(event: Event<Handled>, message: &mut MessageState) -> anyhow::Result<()> {
    message.status = MessageStatus::Handled;
    message.handled_at = Some(event.timestamp);
    message.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_reopened(event: Event<Reopened>, message: &mut MessageState) -> anyhow::Result<()> {
    message.status = MessageStatus::Open;
    message.handled_at = None;
    message.updated_at = event.timestamp;
    Ok(())
}

impl<E: Executor> Command<E> {
    pub async fn load_message(&self, message_id: &str) -> anyhow::Result<Option<MessageState>> {
        message_projection().load(message_id).execute(&self.0).await
    }
}
