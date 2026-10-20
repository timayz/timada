//! Write side: the command gateway and the strict write model it loads.
//!
//! The write model folds *all* events of a subscriber (`strict()`), so the
//! version it observes, and `write()` sends as `original_version`, never
//! goes stale. It is not snapshotted: a subscriber is a handful of events.
//!
//! The write model is sealed, as the events are: [`SubscriberState::open_with`]
//! gives the plaintext [`Subscriber`] while the key exists.

mod campaign;
mod forget;
mod subscriber;

use std::ops::Deref;

pub use campaign::*;
pub use subscriber::*;

use evento::{Executor, Projection, metadata::Event};
use timada_core::keys::{self, Key, Keys};

use crate::aggregator::{Confirmed, Forgotten, Requested, Unsubscribed};
use crate::value_object::{SubscriberStatus, subscriber_id, unsubscribe_token};

/// The command gateway. Cheap to clone when `E` is.
pub struct Command<E: Executor>(pub E);

impl<E: Executor> Deref for Command<E> {
    type Target = E;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

/// The write model: the address stays sealed here.
#[evento::projection(id = id, name = "timada-newsletter/SubscriberState")]
#[evento::snapshot(none)]
#[derive(Debug, PartialEq, Eq)]
pub struct SubscriberState {
    pub id: String,
    /// Names the key `email` is sealed under; empty once forgotten.
    pub key_id: String,
    pub email: Vec<u8>,
    pub language: Option<String>,
    /// Where the last request came from.
    pub source: String,
    pub status: SubscriberStatus,
    /// Unix seconds of the last `Requested`.
    pub requested_at: u64,
    /// Unix seconds of `Confirmed`, while confirmed.
    pub confirmed_at: Option<u64>,
    /// Unix seconds of `Unsubscribed`, while unsubscribed.
    pub unsubscribed_at: Option<u64>,
    /// Unix seconds of the last event.
    pub updated_at: u64,
    /// Erased: the key is destroyed and the address cleared.
    pub forgotten: bool,
}

/// A subscriber, their address opened with their key.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Subscriber {
    pub id: String,
    pub email: String,
    pub language: Option<String>,
    pub source: String,
    pub status: SubscriberStatus,
    pub requested_at: u64,
    pub confirmed_at: Option<u64>,
    pub unsubscribed_at: Option<u64>,
    pub updated_at: u64,
}

impl Subscriber {
    pub fn is_confirmed(&self) -> bool {
        self.status == SubscriberStatus::Confirmed
    }
}

impl SubscriberState {
    pub fn is_confirmed(&self) -> bool {
        !self.forgotten && self.status == SubscriberStatus::Confirmed
    }

    /// Opens the address under `key`, which must be the one `key_id` names.
    pub fn open(&self, key: &Key) -> anyhow::Result<Subscriber> {
        anyhow::ensure!(!self.forgotten, "the subscriber was erased");
        let email = keys::open(key, &email_aad(&self.id, &self.key_id), &self.email)?;
        Ok(Subscriber {
            id: self.id.clone(),
            email: String::from_utf8(email)?,
            language: self.language.clone(),
            source: self.source.clone(),
            status: self.status,
            requested_at: self.requested_at,
            confirmed_at: self.confirmed_at,
            unsubscribed_at: self.unsubscribed_at,
            updated_at: self.updated_at,
        })
    }

    /// Opens the address with the key `keys` holds: `None` once the
    /// subscriber is erased or their key destroyed, an error while the key
    /// cannot be read.
    pub async fn open_with(&self, keys: &dyn Keys) -> anyhow::Result<Option<Subscriber>> {
        match self.key(keys).await? {
            Some(key) => Ok(Some(self.open(&key)?)),
            None => Ok(None),
        }
    }

    /// The link that takes them off the list, for a mail of news to carry:
    /// `None` once erased.
    pub async fn unsubscribe_token(&self, keys: &dyn Keys) -> anyhow::Result<Option<String>> {
        Ok(self
            .key(keys)
            .await?
            .map(|key| unsubscribe_token(&key, &self.id, &self.key_id)))
    }

    async fn key(&self, keys: &dyn Keys) -> anyhow::Result<Option<Key>> {
        if self.forgotten || self.key_id.is_empty() {
            return Ok(None);
        }
        keys.get(&self.key_id).await
    }
}

/// What a sealed address is bound to: the subscriber and their key, so no
/// sealed value can be moved and still open.
pub(crate) fn email_aad(subscriber_id: &str, key_id: &str) -> Vec<u8> {
    keys::aad(subscriber_id, key_id, "email")
}

pub fn subscriber_projection<E: Executor>() -> Projection<E, SubscriberState> {
    Projection::new::<crate::aggregator::Subscriber>()
        .handler(on_requested())
        .handler(on_confirmed())
        .handler(on_unsubscribed())
        .handler(on_forgotten())
        .strict()
}

#[evento::handler]
async fn on_requested(event: Event<Requested>, state: &mut SubscriberState) -> anyhow::Result<()> {
    let data = &event.data;
    state.id = event.aggregate_id.to_owned();
    state.key_id = data.key_id.to_owned();
    state.email = data.email.to_owned();
    state.language = data.language.to_owned();
    state.source = data.source.to_owned();
    state.status = SubscriberStatus::Pending;
    state.requested_at = event.timestamp;
    state.confirmed_at = None;
    state.unsubscribed_at = None;
    state.updated_at = event.timestamp;
    state.forgotten = false;
    Ok(())
}

#[evento::handler]
async fn on_confirmed(event: Event<Confirmed>, state: &mut SubscriberState) -> anyhow::Result<()> {
    state.status = SubscriberStatus::Confirmed;
    state.confirmed_at = Some(event.timestamp);
    state.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_unsubscribed(
    event: Event<Unsubscribed>,
    state: &mut SubscriberState,
) -> anyhow::Result<()> {
    state.status = SubscriberStatus::Unsubscribed;
    state.confirmed_at = None;
    state.unsubscribed_at = Some(event.timestamp);
    state.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_forgotten(event: Event<Forgotten>, state: &mut SubscriberState) -> anyhow::Result<()> {
    state.key_id.clear();
    state.email.clear();
    state.forgotten = true;
    state.updated_at = event.timestamp;
    Ok(())
}

impl<E: Executor> Command<E> {
    /// The sealed write model of a subscriber that is not erased.
    pub async fn load_subscriber(&self, id: &str) -> anyhow::Result<Option<SubscriberState>> {
        Ok(self
            .load_subscriber_any(id)
            .await?
            .filter(|subscriber| !subscriber.forgotten))
    }

    /// The subscriber behind an address (normalized or not), not erased.
    pub async fn subscriber_of(&self, email: &str) -> anyhow::Result<Option<SubscriberState>> {
        let Ok(email) = crate::value_object::normalize_email(email) else {
            return Ok(None);
        };
        self.load_subscriber(&subscriber_id(&email)).await
    }

    /// The sealed write model, erased or not.
    pub(crate) async fn load_subscriber_any(
        &self,
        id: &str,
    ) -> anyhow::Result<Option<SubscriberState>> {
        subscriber_projection().load(id).execute(&self.0).await
    }
}
