//! Write side: the command gateway and the strict write model it loads.
//!
//! The write model folds *all* events of a message (`strict()`), so the
//! version it observes, and `write()` sends as `original_version`, never
//! goes stale. It is not snapshotted: a message is a handful of events.
//!
//! The write model is sealed, as the events are: [`MessageState::open_with`]
//! gives the plaintext [`Message`] while the key exists.

mod forget;
mod message;
mod notification;

use std::ops::Deref;

pub use message::*;

use evento::{Executor, Projection, metadata::Event};
use timada_core::keys::{self, Key, Keys};

use crate::aggregator::{Forgotten, Handled, Notified, Received, Reopened};
use crate::value_object::{MessageStatus, message_notification_id};

/// The command gateway. Cheap to clone when `E` is.
pub struct Command<E: Executor> {
    executor: E,
    tenant: String,
}

impl<E: Executor> Command<E> {
    /// The gateway of one tenant: every stream it writes, loads or reads is
    /// that tenant's. `tenant` must not be empty (the default tenant holds
    /// the platform's own streams).
    pub fn new(executor: E, tenant: impl Into<String>) -> Self {
        let tenant = tenant.into();
        debug_assert!(!tenant.is_empty(), "a tenant gateway needs a tenant");
        Self { executor, tenant }
    }

    /// A gateway naming no tenant: for a store that is one tenant's already
    /// (a storefront's connection, which the store API serves in its key's
    /// tenant whatever its requests name). Every call then acts in the
    /// default tenant, which that store maps to its own.
    pub fn unscoped(executor: E) -> Self {
        Self {
            executor,
            tenant: String::new(),
        }
    }

    /// The tenant every call acts in.
    pub fn tenant(&self) -> &str {
        &self.tenant
    }

    /// The store the tenant's streams live in.
    pub fn executor(&self) -> &E {
        &self.executor
    }
}

impl<E: Executor> Deref for Command<E> {
    type Target = E;

    fn deref(&self) -> &Self::Target {
        &self.executor
    }
}

/// The write model: what the visitor wrote stays sealed here.
#[evento::projection(id = id, name = "timada-support/MessageState")]
#[evento::snapshot(none)]
#[derive(Debug, PartialEq, Eq)]
pub struct MessageState {
    pub id: String,
    /// Names the key the fields below are sealed under; empty once forgotten.
    pub key_id: String,
    pub first_name: Vec<u8>,
    pub last_name: Vec<u8>,
    pub email: Vec<u8>,
    pub body: Vec<u8>,
    pub language: Option<String>,
    pub status: MessageStatus,
    /// Unix seconds of `Received`.
    pub received_at: u64,
    /// Unix seconds of `Handled`, while it is.
    pub handled_at: Option<u64>,
    /// Unix seconds of the last event.
    pub updated_at: u64,
    /// Erased: the key is destroyed and the sealed fields cleared.
    pub forgotten: bool,
}

/// A message as the visitor wrote it, opened with its key.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Message {
    pub id: String,
    pub first_name: String,
    pub last_name: String,
    pub email: String,
    pub body: String,
    pub language: Option<String>,
    pub status: MessageStatus,
    pub received_at: u64,
    pub handled_at: Option<u64>,
    pub updated_at: u64,
}

impl Message {
    pub fn is_handled(&self) -> bool {
        self.status == MessageStatus::Handled
    }

    /// `Ada Lovelace`.
    pub fn full_name(&self) -> String {
        format!("{} {}", self.first_name, self.last_name)
    }
}

impl MessageState {
    pub fn is_handled(&self) -> bool {
        self.status == MessageStatus::Handled
    }

    /// Opens the fields under `key`, which must be the one `key_id` names.
    pub fn open(&self, key: &Key) -> anyhow::Result<Message> {
        anyhow::ensure!(!self.forgotten, "the message was erased");
        let field = |name: &str, sealed: &[u8]| -> anyhow::Result<String> {
            let bytes = keys::open(key, &field_aad(&self.id, &self.key_id, name), sealed)?;
            Ok(String::from_utf8(bytes)?)
        };
        Ok(Message {
            id: self.id.clone(),
            first_name: field(FIRST_NAME, &self.first_name)?,
            last_name: field(LAST_NAME, &self.last_name)?,
            email: field(EMAIL, &self.email)?,
            body: field(BODY, &self.body)?,
            language: self.language.clone(),
            status: self.status,
            received_at: self.received_at,
            handled_at: self.handled_at,
            updated_at: self.updated_at,
        })
    }

    /// Opens the fields with the key `keys` holds: `None` once the message is
    /// erased or its key destroyed, an error while the key cannot be read.
    pub async fn open_with(&self, keys: &dyn Keys) -> anyhow::Result<Option<Message>> {
        if self.forgotten || self.key_id.is_empty() {
            return Ok(None);
        }
        match keys.get(&self.key_id).await? {
            Some(key) => Ok(Some(self.open(&key)?)),
            None => Ok(None),
        }
    }
}

pub(crate) const FIRST_NAME: &str = "first_name";
pub(crate) const LAST_NAME: &str = "last_name";
pub(crate) const EMAIL: &str = "email";
pub(crate) const BODY: &str = "body";

/// What a field is bound to when sealed: the message, its key and the
/// field's name, so no sealed value can be moved and still open.
pub(crate) fn field_aad(message_id: &str, key_id: &str, field: &str) -> Vec<u8> {
    keys::aad(message_id, key_id, field)
}

pub fn message_projection<E: Executor>() -> Projection<E, MessageState> {
    Projection::new::<crate::aggregator::Message>()
        .handler(on_received())
        .handler(on_handled())
        .handler(on_reopened())
        .handler(on_forgotten())
        .strict()
}

#[evento::handler]
async fn on_received(event: Event<Received>, message: &mut MessageState) -> anyhow::Result<()> {
    let data = &event.data;
    message.id = event.aggregate_id.to_owned();
    message.key_id = data.key_id.to_owned();
    message.first_name = data.first_name.to_owned();
    message.last_name = data.last_name.to_owned();
    message.email = data.email.to_owned();
    message.body = data.body.to_owned();
    message.language = data.language.to_owned();
    message.status = MessageStatus::Open;
    message.received_at = event.timestamp;
    message.updated_at = event.timestamp;
    message.forgotten = false;
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

#[evento::handler]
async fn on_forgotten(event: Event<Forgotten>, message: &mut MessageState) -> anyhow::Result<()> {
    message.key_id.clear();
    message.first_name.clear();
    message.last_name.clear();
    message.email.clear();
    message.body.clear();
    message.forgotten = true;
    message.updated_at = event.timestamp;
    Ok(())
}

/// Whether, and when, the merchant was told of a message.
#[evento::projection(id = id, name = "timada-support/MessageNotificationState")]
#[evento::snapshot(none)]
#[derive(Debug, PartialEq, Eq)]
pub struct MessageNotificationState {
    pub id: String,
    pub message_id: String,
    pub mailed_by: String,
    /// Unix seconds.
    pub mailed_at: u64,
}

pub fn message_notification_projection<E: Executor>() -> Projection<E, MessageNotificationState> {
    Projection::new::<crate::aggregator::MessageNotification>()
        .handler(on_notified())
        .strict()
}

#[evento::handler]
async fn on_notified(
    event: Event<Notified>,
    state: &mut MessageNotificationState,
) -> anyhow::Result<()> {
    state.id = event.aggregate_id.to_owned();
    state.message_id = event.data.message_id.clone();
    state.mailed_by = event.data.mailed_by.clone();
    state.mailed_at = event.timestamp;
    Ok(())
}

impl<E: Executor> Command<E> {
    /// The notice of a message's arrival, once a node claimed it.
    pub async fn load_message_notification(
        &self,
        message_id: &str,
    ) -> anyhow::Result<Option<MessageNotificationState>> {
        message_notification_projection()
            .load(message_notification_id(message_id))
            .tenant(&self.tenant)
            .execute(&self.executor)
            .await
    }

    /// The sealed write model of a message that is not erased.
    pub async fn load_message(&self, message_id: &str) -> anyhow::Result<Option<MessageState>> {
        Ok(self
            .load_message_any(message_id)
            .await?
            .filter(|message| !message.forgotten))
    }

    /// The sealed write model, erased or not.
    pub(crate) async fn load_message_any(
        &self,
        message_id: &str,
    ) -> anyhow::Result<Option<MessageState>> {
        message_projection()
            .load(message_id)
            .tenant(&self.tenant)
            .execute(&self.executor)
            .await
    }
}
