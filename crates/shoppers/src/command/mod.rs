//! Write side: the command gateway and the strict write models it loads.
//!
//! Each write model folds *all* events of its stream (`strict()`), so the
//! version it observes, and `write()` sends as `original_version`, never goes
//! stale. None is snapshotted: every stream stays short.

mod address;
mod challenge;
mod forget;
mod session;

use std::ops::Deref;

pub use challenge::*;
pub use session::*;

use evento::{Executor, Projection, metadata::Event};
use timada_orders::ShippingAddress;

use crate::aggregator::{
    AddressRemoved, AddressSaved, AttemptFailed, CodeConsumed, CodeIssued, Forgotten, Registered,
    SessionEnded, SessionStarted, SessionsRevoked,
};
use crate::value_object::{challenge_id, session_id, shopper_id};
use timada_core::keys::{self as key, Key as ShopperKey, Keys as ShopperKeys};

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

/// An address a shopper ships to, opened.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SavedAddress {
    pub id: String,
    pub address: ShippingAddress,
    /// Unix seconds.
    pub saved_at: u64,
}

/// An address a shopper ships to, as the events hold it: sealed under the
/// shopper's key.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SealedAddress {
    pub id: String,
    pub sealed: Vec<u8>,
    /// Unix seconds.
    pub saved_at: u64,
}

/// A shopper with their personal data opened by their key.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Shopper {
    pub id: String,
    /// Normalized.
    pub email: String,
    /// Unix seconds of the first sign-in (since the last deletion).
    pub registered_at: u64,
    /// Most recently saved first.
    pub addresses: Vec<SavedAddress>,
}

/// The latest "sign out everywhere" on a shopper. Sessions started up to it
/// are void; the one it names (the browser that asked) survives.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SessionsRevocation {
    /// Unix milliseconds of the revocation.
    pub at_ms: u64,
    /// Session aggregate id spared by the revocation.
    pub except_session: Option<String>,
}

impl SessionsRevocation {
    pub fn covers(&self, session: &SessionState) -> bool {
        session.started_at_ms <= self.at_ms
            && self.except_session.as_deref() != Some(session.id.as_str())
    }
}

/// Unix milliseconds of an event, for cutoffs where a second is too coarse.
fn event_millis(event: &evento::Event) -> u64 {
    event.timestamp * 1000 + u64::from(event.timestamp_subsec)
}

#[evento::projection(id = id, name = "timada-shoppers/ShopperState")]
#[evento::snapshot(none)]
#[derive(Debug, PartialEq, Eq)]
pub struct ShopperState {
    pub id: String,
    /// The key the personal data is sealed under; empty once forgotten.
    pub key_id: String,
    /// The normalized address signed in with, sealed; empty once forgotten.
    pub email: Vec<u8>,
    /// Unix seconds of the first sign-in (since the last deletion).
    pub registered_at: u64,
    /// Most recently saved first.
    pub addresses: Vec<SealedAddress>,
    pub sessions_revoked: Option<SessionsRevocation>,
    /// The shopper deleted their account; every command treats it as gone
    /// until the address signs in again.
    pub forgotten: bool,
}

impl Shopper {
    pub fn address(&self, address_id: &str) -> Option<&SavedAddress> {
        self.addresses.iter().find(|saved| saved.id == address_id)
    }
}

impl ShopperState {
    pub fn address(&self, address_id: &str) -> Option<&SealedAddress> {
        self.addresses.iter().find(|saved| saved.id == address_id)
    }

    /// The personal data, opened with the key `key_id` names. Fails under
    /// any other key.
    pub fn open(&self, key: &ShopperKey) -> anyhow::Result<Shopper> {
        let email = key::open(key, &email_aad(&self.id, &self.key_id), &self.email)?;
        let addresses = self
            .addresses
            .iter()
            .map(|saved| {
                let bytes = key::open(
                    key,
                    &address_aad(&self.id, &self.key_id, &saved.id),
                    &saved.sealed,
                )?;
                Ok(SavedAddress {
                    id: saved.id.to_owned(),
                    address: bitcode::decode(&bytes)?,
                    saved_at: saved.saved_at,
                })
            })
            .collect::<anyhow::Result<_>>()?;
        Ok(Shopper {
            id: self.id.to_owned(),
            email: String::from_utf8(email)?,
            registered_at: self.registered_at,
            addresses,
        })
    }
}

impl ShopperState {
    /// The personal data, opened with the key `keys` hold: `None` once the
    /// shopper is forgotten or the key destroyed; an error when the key
    /// cannot be read yet.
    pub async fn open_with(&self, keys: &dyn ShopperKeys) -> anyhow::Result<Option<Shopper>> {
        if self.forgotten || self.key_id.is_empty() {
            return Ok(None);
        }
        let Some(key) = keys.get(&self.key_id).await? else {
            return Ok(None);
        };
        self.open(&key).map(Some)
    }
}

/// What a sealed address signed in with is bound to.
pub(crate) fn email_aad(shopper_id: &str, key_id: &str) -> Vec<u8> {
    key::aad(shopper_id, key_id, "email")
}

/// What a sealed saved address is bound to.
pub(crate) fn address_aad(shopper_id: &str, key_id: &str, address_id: &str) -> Vec<u8> {
    key::aad(shopper_id, key_id, &format!("address:{address_id}"))
}

pub fn shopper_projection<E: Executor>() -> Projection<E, ShopperState> {
    Projection::new::<crate::aggregator::Shopper>()
        .handler(on_registered())
        .handler(on_address_saved())
        .handler(on_address_removed())
        .handler(on_sessions_revoked())
        .handler(on_forgotten())
        .strict()
}

#[evento::handler]
async fn on_registered(event: Event<Registered>, state: &mut ShopperState) -> anyhow::Result<()> {
    state.id = event.aggregate_id.to_owned();
    state.key_id = event.data.key_id.to_owned();
    state.email = event.data.email.to_owned();
    state.registered_at = event.timestamp;
    state.addresses.clear();
    state.forgotten = false;
    Ok(())
}

#[evento::handler]
async fn on_address_saved(
    event: Event<AddressSaved>,
    state: &mut ShopperState,
) -> anyhow::Result<()> {
    state
        .addresses
        .retain(|saved| saved.id != event.data.address_id);
    state.addresses.insert(
        0,
        SealedAddress {
            id: event.data.address_id.to_owned(),
            sealed: event.data.address.to_owned(),
            saved_at: event.timestamp,
        },
    );
    Ok(())
}

#[evento::handler]
async fn on_address_removed(
    event: Event<AddressRemoved>,
    state: &mut ShopperState,
) -> anyhow::Result<()> {
    state
        .addresses
        .retain(|saved| saved.id != event.data.address_id);
    Ok(())
}

#[evento::handler]
async fn on_sessions_revoked(
    event: Event<SessionsRevoked>,
    state: &mut ShopperState,
) -> anyhow::Result<()> {
    state.sessions_revoked = Some(SessionsRevocation {
        at_ms: event_millis(&event),
        except_session: event.data.except_session.to_owned(),
    });
    Ok(())
}

#[evento::handler]
async fn on_forgotten(event: Event<Forgotten>, state: &mut ShopperState) -> anyhow::Result<()> {
    state.key_id.clear();
    state.email.clear();
    state.addresses.clear();
    state.forgotten = true;
    state.sessions_revoked = Some(SessionsRevocation {
        at_ms: event_millis(&event),
        except_session: None,
    });
    Ok(())
}

#[evento::projection(id = id, name = "timada-shoppers/ChallengeState")]
#[evento::snapshot(none)]
#[derive(Debug, PartialEq, Eq)]
pub struct ChallengeState {
    pub id: String,
    pub code_hash: String,
    /// Unix seconds.
    pub issued_at: u64,
    /// Unix seconds.
    pub expires_at: u64,
    /// Wrong guesses against the current code.
    pub failed_attempts: u32,
    /// The current code was used.
    pub consumed: bool,
}

pub fn challenge_projection<E: Executor>() -> Projection<E, ChallengeState> {
    Projection::new::<crate::aggregator::ShopperChallenge>()
        .handler(on_code_issued())
        .handler(on_attempt_failed())
        .handler(on_code_consumed())
        .strict()
}

#[evento::handler]
async fn on_code_issued(
    event: Event<CodeIssued>,
    state: &mut ChallengeState,
) -> anyhow::Result<()> {
    state.id = event.aggregate_id.to_owned();
    state.code_hash = event.data.code_hash.to_owned();
    state.issued_at = event.data.issued_at;
    state.expires_at = event.data.expires_at;
    state.failed_attempts = 0;
    state.consumed = false;
    Ok(())
}

#[evento::handler]
async fn on_attempt_failed(
    _event: Event<AttemptFailed>,
    state: &mut ChallengeState,
) -> anyhow::Result<()> {
    state.failed_attempts += 1;
    Ok(())
}

#[evento::handler]
async fn on_code_consumed(
    _event: Event<CodeConsumed>,
    state: &mut ChallengeState,
) -> anyhow::Result<()> {
    state.consumed = true;
    Ok(())
}

#[evento::projection(id = id, name = "timada-shoppers/SessionState")]
#[evento::snapshot(none)]
#[derive(Debug, PartialEq, Eq)]
pub struct SessionState {
    pub id: String,
    pub shopper_id: String,
    /// Unix milliseconds, for the revocation cutoff.
    pub started_at_ms: u64,
    /// Unix seconds.
    pub expires_at: u64,
    pub ended: bool,
}

impl SessionState {
    pub fn is_valid_at(&self, now: u64) -> bool {
        !self.ended && now < self.expires_at
    }
}

pub fn session_projection<E: Executor>() -> Projection<E, SessionState> {
    Projection::new::<crate::aggregator::ShopperSession>()
        .handler(on_session_started())
        .handler(on_session_ended())
        .strict()
}

#[evento::handler]
async fn on_session_started(
    event: Event<SessionStarted>,
    state: &mut SessionState,
) -> anyhow::Result<()> {
    state.id = event.aggregate_id.to_owned();
    state.shopper_id = event.data.shopper_id.to_owned();
    state.started_at_ms = event_millis(&event);
    state.expires_at = event.data.expires_at;
    state.ended = false;
    Ok(())
}

#[evento::handler]
async fn on_session_ended(
    _event: Event<SessionEnded>,
    state: &mut SessionState,
) -> anyhow::Result<()> {
    state.ended = true;
    Ok(())
}

impl<E: Executor> Command<E> {
    /// The shopper behind a normalized address, once one signed in with it.
    pub async fn load_shopper_by_email(&self, email: &str) -> anyhow::Result<Option<ShopperState>> {
        self.load_shopper(&shopper_id(email)).await
    }

    /// The shopper, or `None` when unknown or forgotten.
    pub async fn load_shopper(&self, shopper_id: &str) -> anyhow::Result<Option<ShopperState>> {
        Ok(self
            .load_shopper_any(shopper_id)
            .await?
            .filter(|shopper| !shopper.forgotten))
    }

    /// The shopper including a forgotten one, whose stream a new sign-in
    /// registers again.
    pub(crate) async fn load_shopper_any(
        &self,
        shopper_id: &str,
    ) -> anyhow::Result<Option<ShopperState>> {
        shopper_projection()
            .load(shopper_id)
            .tenant(&self.tenant)
            .execute(&self.executor)
            .await
    }

    pub(crate) async fn load_challenge(
        &self,
        email: &str,
    ) -> anyhow::Result<Option<ChallengeState>> {
        challenge_projection()
            .load(challenge_id(email))
            .tenant(&self.tenant)
            .execute(&self.executor)
            .await
    }

    pub(crate) async fn load_session(&self, token: &str) -> anyhow::Result<Option<SessionState>> {
        session_projection()
            .load(session_id(token))
            .tenant(&self.tenant)
            .execute(&self.executor)
            .await
    }
}
