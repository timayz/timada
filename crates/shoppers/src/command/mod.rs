//! Write side: the command gateway and the strict write models it loads.
//!
//! Each write model folds *all* events of its stream (`strict()`), so the
//! version it observes, and `write()` sends as `original_version`, never goes
//! stale. None is snapshotted: every stream stays short.

mod address;
mod challenge;
mod session;

use std::ops::Deref;

pub use challenge::*;
pub use session::*;

use evento::{Executor, Projection, metadata::Event};
use timada_orders::ShippingAddress;

use crate::aggregator::{
    AddressRemoved, AddressSaved, AttemptFailed, CodeConsumed, CodeIssued, Registered,
    SessionEnded, SessionStarted,
};
use crate::value_object::{challenge_id, session_id, shopper_id};

/// The command gateway. Cheap to clone when `E` is.
pub struct Command<E: Executor>(pub E);

impl<E: Executor> Deref for Command<E> {
    type Target = E;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

/// An address a shopper ships to.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SavedAddress {
    pub id: String,
    pub address: ShippingAddress,
    /// Unix seconds.
    pub saved_at: u64,
}

#[evento::projection(id = id, name = "timada-shoppers/ShopperState")]
#[evento::snapshot(none)]
#[derive(Debug, PartialEq, Eq)]
pub struct ShopperState {
    pub id: String,
    /// Normalized.
    pub email: String,
    /// Unix seconds of the first sign-in.
    pub registered_at: u64,
    /// Most recently saved first.
    pub addresses: Vec<SavedAddress>,
}

impl ShopperState {
    pub fn address(&self, address_id: &str) -> Option<&SavedAddress> {
        self.addresses.iter().find(|saved| saved.id == address_id)
    }
}

pub fn shopper_projection<E: Executor>() -> Projection<E, ShopperState> {
    Projection::new::<crate::aggregator::Shopper>()
        .handler(on_registered())
        .handler(on_address_saved())
        .handler(on_address_removed())
        .strict()
}

#[evento::handler]
async fn on_registered(event: Event<Registered>, state: &mut ShopperState) -> anyhow::Result<()> {
    state.id = event.aggregate_id.to_owned();
    state.email = event.data.email.to_owned();
    state.registered_at = event.timestamp;
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
        SavedAddress {
            id: event.data.address_id.to_owned(),
            address: event.data.address.to_owned(),
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

    pub async fn load_shopper(&self, shopper_id: &str) -> anyhow::Result<Option<ShopperState>> {
        shopper_projection().load(shopper_id).execute(&self.0).await
    }

    pub(crate) async fn load_challenge(
        &self,
        email: &str,
    ) -> anyhow::Result<Option<ChallengeState>> {
        challenge_projection()
            .load(challenge_id(email))
            .execute(&self.0)
            .await
    }

    pub(crate) async fn load_session(&self, token: &str) -> anyhow::Result<Option<SessionState>> {
        session_projection()
            .load(session_id(token))
            .execute(&self.0)
            .await
    }
}
