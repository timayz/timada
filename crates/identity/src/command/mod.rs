//! Write side: the command gateway and the strict write models it loads.
//!
//! Every write model folds *all* events of its aggregate (`strict()`), so the
//! version it observes, and `write()` sends as `original_version`, never goes
//! stale. None of them is snapshotted: streams are a handful of events.

mod complete_oauth2;
mod end_session;
mod first_user;
mod oauth2_index;
mod passkey_index;
mod refresh_session;
mod register_passkey;
mod remove_passkey;
mod rename_passkey;
mod resolve_session;
mod revoke_sessions;
mod sign_in;
mod sign_in_passkey;
mod start_session;
mod unlink_oauth2;
mod update_profile;

use std::ops::Deref;

pub use complete_oauth2::*;
pub use register_passkey::*;
pub use resolve_session::*;
pub use sign_in_passkey::*;

use evento::{
    Executor, Projection, ProjectionAggregate, WriteBuilder, WriteError, metadata::Event,
};

use crate::aggregator::{
    AdminGranted, Created, Deleted, Ended, FirstUserClaimed, Linked, OAuth2AccountLinked,
    OAuth2AccountUnlinked, PasskeyAdded, PasskeyRemoved, PasskeyRenamed, ProfileUpdated, Refreshed,
    Registered, Removed, SessionsRevoked, SignedIn, SignedOut, Started, Unlinked, Updated,
};
use crate::error::IdentityError;
use crate::value_object::{
    REGISTRY_ID, SignInMethod, oauth2_account_id, passkey_credential_id, session_id,
};

/// The command gateway. Cheap to clone when `E` is (the dashboard's executor
/// is an `Arc` wrapper).
pub struct Command<E: Executor>(pub E);

impl<E: Executor> Deref for Command<E> {
    type Target = E;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

/// Reload-and-retry budget for appends to a `User` stream that raced another
/// writer (two tabs linking at once). The index aggregates never retry: a
/// version-0 conflict there *is* the answer.
const USER_APPEND_ATTEMPTS: usize = 3;

// =============================================================================
// User
// =============================================================================

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LinkedOAuth2Account {
    pub provider: String,
    pub provider_user_id: String,
    pub name: Option<String>,
    pub email: Option<String>,
    pub picture: Option<String>,
    /// Unix seconds.
    pub linked_at: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LinkedPasskey {
    pub credential_id: String,
    pub name: String,
    pub aaguid: String,
    /// Unix seconds.
    pub added_at: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SignInRecord {
    pub method: SignInMethod,
    pub provider: Option<String>,
    pub credential_id: Option<String>,
    pub user_agent: Option<String>,
    pub ip: Option<String>,
    /// Unix seconds.
    pub at: u64,
}

/// Sign-ins kept on the user for the account page's history.
const SIGN_IN_HISTORY: usize = 50;

/// The latest "sign out everywhere" on a user. Sessions started before it are
/// void; the one it names (the browser that asked) survives.
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

#[evento::projection(id = id, name = "timada-identity/UserState")]
#[evento::snapshot(none)]
#[derive(Debug, PartialEq, Eq)]
pub struct UserState {
    pub id: String,
    pub account: String,
    pub label: String,
    pub is_admin: bool,
    pub oauth2_accounts: Vec<LinkedOAuth2Account>,
    pub passkeys: Vec<LinkedPasskey>,
    /// Most recent last.
    pub sign_ins: Vec<SignInRecord>,
    /// Unix seconds of `Registered`.
    pub registered_at: u64,
    /// The stream stays replayable but every command treats the user as gone.
    pub deleted: bool,
    pub sessions_revoked: Option<SessionsRevocation>,
}

impl UserState {
    /// How many ways this user can still sign in. Unlinking the last one would
    /// lock them out.
    pub fn sign_in_methods(&self) -> usize {
        self.oauth2_accounts.len() + self.passkeys.len()
    }

    pub fn has_oauth2_account(&self, provider: &str, provider_user_id: &str) -> bool {
        self.oauth2_accounts
            .iter()
            .any(|a| a.provider == provider && a.provider_user_id == provider_user_id)
    }

    pub fn passkey(&self, credential_id: &str) -> Option<&LinkedPasskey> {
        self.passkeys
            .iter()
            .find(|p| p.credential_id == credential_id)
    }

    pub fn last_sign_in(&self) -> Option<&SignInRecord> {
        self.sign_ins.last()
    }
}

pub fn user_projection<E: Executor>() -> Projection<E, UserState> {
    Projection::new::<crate::aggregator::User>()
        .handler(on_registered())
        .handler(on_profile_updated())
        .handler(on_admin_granted())
        .handler(on_oauth2_account_linked())
        .handler(on_oauth2_account_unlinked())
        .handler(on_passkey_added())
        .handler(on_passkey_renamed())
        .handler(on_passkey_removed())
        .handler(on_signed_in())
        .handler(on_deleted())
        .handler(on_sessions_revoked())
        .skip::<SignedOut>()
        .strict()
}

#[evento::handler]
async fn on_registered(event: Event<Registered>, user: &mut UserState) -> anyhow::Result<()> {
    user.id = event.aggregate_id.to_owned();
    user.account = event.data.account.to_owned();
    user.label = event.data.label.to_owned();
    user.registered_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_profile_updated(
    event: Event<ProfileUpdated>,
    user: &mut UserState,
) -> anyhow::Result<()> {
    user.account = event.data.account.to_owned();
    user.label = event.data.label.to_owned();
    Ok(())
}

#[evento::handler]
async fn on_admin_granted(_event: Event<AdminGranted>, user: &mut UserState) -> anyhow::Result<()> {
    user.is_admin = true;
    Ok(())
}

#[evento::handler]
async fn on_oauth2_account_linked(
    event: Event<OAuth2AccountLinked>,
    user: &mut UserState,
) -> anyhow::Result<()> {
    let data = &event.data;
    user.oauth2_accounts
        .retain(|a| !(a.provider == data.provider && a.provider_user_id == data.provider_user_id));
    user.oauth2_accounts.push(LinkedOAuth2Account {
        provider: data.provider.to_owned(),
        provider_user_id: data.provider_user_id.to_owned(),
        name: data.name.to_owned(),
        email: data.email.to_owned(),
        picture: data.picture.to_owned(),
        linked_at: event.timestamp,
    });
    Ok(())
}

#[evento::handler]
async fn on_oauth2_account_unlinked(
    event: Event<OAuth2AccountUnlinked>,
    user: &mut UserState,
) -> anyhow::Result<()> {
    let data = &event.data;
    user.oauth2_accounts
        .retain(|a| !(a.provider == data.provider && a.provider_user_id == data.provider_user_id));
    Ok(())
}

#[evento::handler]
async fn on_passkey_added(event: Event<PasskeyAdded>, user: &mut UserState) -> anyhow::Result<()> {
    user.passkeys
        .retain(|p| p.credential_id != event.data.credential_id);
    user.passkeys.push(LinkedPasskey {
        credential_id: event.data.credential_id.to_owned(),
        name: event.data.name.to_owned(),
        aaguid: event.data.aaguid.to_owned(),
        added_at: event.timestamp,
    });
    Ok(())
}

#[evento::handler]
async fn on_passkey_renamed(
    event: Event<PasskeyRenamed>,
    user: &mut UserState,
) -> anyhow::Result<()> {
    if let Some(passkey) = user
        .passkeys
        .iter_mut()
        .find(|p| p.credential_id == event.data.credential_id)
    {
        passkey.name = event.data.name.to_owned();
    }
    Ok(())
}

#[evento::handler]
async fn on_passkey_removed(
    event: Event<PasskeyRemoved>,
    user: &mut UserState,
) -> anyhow::Result<()> {
    user.passkeys
        .retain(|p| p.credential_id != event.data.credential_id);
    Ok(())
}

#[evento::handler]
async fn on_signed_in(event: Event<SignedIn>, user: &mut UserState) -> anyhow::Result<()> {
    user.sign_ins.push(SignInRecord {
        method: event.data.method,
        provider: event.data.provider.to_owned(),
        credential_id: event.data.credential_id.to_owned(),
        user_agent: event.data.user_agent.to_owned(),
        ip: event.data.ip.to_owned(),
        at: event.timestamp,
    });
    if user.sign_ins.len() > SIGN_IN_HISTORY {
        let excess = user.sign_ins.len() - SIGN_IN_HISTORY;
        user.sign_ins.drain(..excess);
    }
    Ok(())
}

#[evento::handler]
async fn on_deleted(_event: Event<Deleted>, user: &mut UserState) -> anyhow::Result<()> {
    user.deleted = true;
    Ok(())
}

#[evento::handler]
async fn on_sessions_revoked(
    event: Event<SessionsRevoked>,
    user: &mut UserState,
) -> anyhow::Result<()> {
    user.sessions_revoked = Some(SessionsRevocation {
        at_ms: event_millis(&event),
        except_session: event.data.except_session.to_owned(),
    });
    Ok(())
}

// =============================================================================
// OAuth2Account index
// =============================================================================

#[evento::projection(id = id, name = "timada-identity/OAuth2AccountState")]
#[evento::snapshot(none)]
#[derive(Debug, PartialEq, Eq)]
pub struct OAuth2AccountState {
    pub id: String,
    pub user_id: String,
    pub provider: String,
    pub provider_user_id: String,
    pub unlinked: bool,
}

pub fn oauth2_account_projection<E: Executor>() -> Projection<E, OAuth2AccountState> {
    Projection::new::<crate::aggregator::OAuth2Account>()
        .handler(on_linked())
        .handler(on_unlinked())
        .strict()
}

#[evento::handler]
async fn on_linked(event: Event<Linked>, state: &mut OAuth2AccountState) -> anyhow::Result<()> {
    state.id = event.aggregate_id.to_owned();
    state.user_id = event.data.user_id.to_owned();
    state.provider = event.data.provider.to_owned();
    state.provider_user_id = event.data.provider_user_id.to_owned();
    state.unlinked = false;
    Ok(())
}

#[evento::handler]
async fn on_unlinked(
    _event: Event<Unlinked>,
    state: &mut OAuth2AccountState,
) -> anyhow::Result<()> {
    state.unlinked = true;
    Ok(())
}

// =============================================================================
// PasskeyCredential index
// =============================================================================

#[evento::projection(id = id, name = "timada-identity/PasskeyCredentialState")]
#[evento::snapshot(none)]
#[derive(Debug, PartialEq, Eq)]
pub struct PasskeyCredentialState {
    pub id: String,
    pub user_id: String,
    pub credential_id: String,
    pub passkey_json: String,
    pub aaguid: String,
    pub rp_id: String,
    pub name: String,
    pub removed: bool,
}

pub fn passkey_credential_projection<E: Executor>() -> Projection<E, PasskeyCredentialState> {
    Projection::new::<crate::aggregator::PasskeyCredential>()
        .handler(on_passkey_created())
        .handler(on_passkey_updated())
        .handler(on_passkey_credential_removed())
        .strict()
}

#[evento::handler]
async fn on_passkey_created(
    event: Event<Created>,
    state: &mut PasskeyCredentialState,
) -> anyhow::Result<()> {
    state.id = event.aggregate_id.to_owned();
    state.user_id = event.data.user_id.to_owned();
    state.credential_id = event.data.credential_id.to_owned();
    state.passkey_json = event.data.passkey_json.to_owned();
    state.aaguid = event.data.aaguid.to_owned();
    state.rp_id = event.data.rp_id.to_owned();
    state.name = event.data.name.to_owned();
    state.removed = false;
    Ok(())
}

#[evento::handler]
async fn on_passkey_updated(
    event: Event<Updated>,
    state: &mut PasskeyCredentialState,
) -> anyhow::Result<()> {
    state.passkey_json = event.data.passkey_json.to_owned();
    Ok(())
}

#[evento::handler]
async fn on_passkey_credential_removed(
    _event: Event<Removed>,
    state: &mut PasskeyCredentialState,
) -> anyhow::Result<()> {
    state.removed = true;
    Ok(())
}

// =============================================================================
// Session
// =============================================================================

#[evento::projection(id = id, name = "timada-identity/SessionState")]
#[evento::snapshot(none)]
#[derive(Debug, PartialEq, Eq)]
pub struct SessionState {
    pub id: String,
    pub user_id: String,
    /// Unix seconds of `Started`.
    pub started_at: u64,
    /// Unix milliseconds of `Started`, compared against a revocation cutoff.
    pub started_at_ms: u64,
    /// Unix seconds.
    pub expires_at: u64,
    /// Unix seconds of the latest `Refreshed`.
    pub refreshed_at: Option<u64>,
    pub ended: bool,
    pub user_agent: Option<String>,
    pub ip: Option<String>,
}

impl SessionState {
    pub fn is_valid_at(&self, now: u64) -> bool {
        !self.ended && self.expires_at > now
    }
}

pub fn session_projection<E: Executor>() -> Projection<E, SessionState> {
    Projection::new::<crate::aggregator::Session>()
        .handler(on_session_started())
        .handler(on_session_refreshed())
        .handler(on_session_ended())
        .strict()
}

#[evento::handler]
async fn on_session_started(event: Event<Started>, state: &mut SessionState) -> anyhow::Result<()> {
    state.id = event.aggregate_id.to_owned();
    state.user_id = event.data.user_id.to_owned();
    state.started_at = event.timestamp;
    state.started_at_ms = event_millis(&event);
    state.expires_at = event.data.expires_at;
    state.ended = false;
    state.user_agent = event.data.user_agent.to_owned();
    state.ip = event.data.ip.to_owned();
    Ok(())
}

#[evento::handler]
async fn on_session_refreshed(
    event: Event<Refreshed>,
    state: &mut SessionState,
) -> anyhow::Result<()> {
    state.expires_at = event.data.expires_at;
    state.refreshed_at = Some(event.timestamp);
    Ok(())
}

#[evento::handler]
async fn on_session_ended(_event: Event<Ended>, state: &mut SessionState) -> anyhow::Result<()> {
    state.ended = true;
    Ok(())
}

// =============================================================================
// Registry
// =============================================================================

#[evento::projection(id = id, name = "timada-identity/RegistryState")]
#[evento::snapshot(none)]
#[derive(Debug, PartialEq, Eq)]
pub struct RegistryState {
    pub id: String,
    pub first_user_id: String,
}

pub fn registry_projection<E: Executor>() -> Projection<E, RegistryState> {
    Projection::new::<crate::aggregator::Registry>()
        .handler(on_first_user_claimed())
        .strict()
}

#[evento::handler]
async fn on_first_user_claimed(
    event: Event<FirstUserClaimed>,
    state: &mut RegistryState,
) -> anyhow::Result<()> {
    state.id = event.aggregate_id.to_owned();
    state.first_user_id = event.data.user_id.to_owned();
    Ok(())
}

// =============================================================================
// Loads and shared write helpers
// =============================================================================

impl<E: Executor> Command<E> {
    /// The user, or `None` when unknown or deleted.
    pub async fn load_user(&self, id: impl Into<String>) -> anyhow::Result<Option<UserState>> {
        Ok(user_projection()
            .load(id)
            .execute(&self.0)
            .await?
            .filter(|u| !u.deleted))
    }

    /// The currently linked index entry for a provider identity.
    pub async fn load_oauth2_account(
        &self,
        provider: &str,
        provider_user_id: &str,
    ) -> anyhow::Result<Option<OAuth2AccountState>> {
        Ok(self
            .load_oauth2_account_any(provider, provider_user_id)
            .await?
            .filter(|a| !a.unlinked))
    }

    /// The index entry including unlinked ones (needed to re-link).
    pub(crate) async fn load_oauth2_account_any(
        &self,
        provider: &str,
        provider_user_id: &str,
    ) -> anyhow::Result<Option<OAuth2AccountState>> {
        oauth2_account_projection()
            .load(oauth2_account_id(provider, provider_user_id))
            .execute(&self.0)
            .await
    }

    /// The registered passkey for a base64url credential id.
    pub async fn load_passkey(
        &self,
        credential_id: &str,
    ) -> anyhow::Result<Option<PasskeyCredentialState>> {
        Ok(self
            .load_passkey_any(credential_id)
            .await?
            .filter(|p| !p.removed))
    }

    pub(crate) async fn load_passkey_any(
        &self,
        credential_id: &str,
    ) -> anyhow::Result<Option<PasskeyCredentialState>> {
        passkey_credential_projection()
            .load(passkey_credential_id(credential_id))
            .execute(&self.0)
            .await
    }

    pub async fn load_session(
        &self,
        token_hash: &[u8; 32],
    ) -> anyhow::Result<Option<SessionState>> {
        session_projection()
            .load(session_id(token_hash))
            .execute(&self.0)
            .await
    }

    pub async fn load_registry(&self) -> anyhow::Result<Option<RegistryState>> {
        registry_projection()
            .load(REGISTRY_ID)
            .execute(&self.0)
            .await
    }

    /// Appends to a user's stream with reload-and-retry on a version race.
    /// `build` runs on every attempt against the fresh state: re-check guards
    /// there and add the events to the builder. Returns the state the events
    /// were appended onto.
    pub(crate) async fn append_user<F>(
        &self,
        user_id: &str,
        routing_key: Option<String>,
        build: F,
    ) -> Result<UserState, IdentityError>
    where
        F: Fn(&UserState, &mut WriteBuilder) -> Result<(), IdentityError>,
    {
        let mut attempt = 0;
        loop {
            attempt += 1;
            let Some(user) = self.load_user(user_id).await? else {
                return Err(IdentityError::UserNotFound);
            };
            let mut builder = user.write()?;
            builder.routing_key_opt(routing_key.clone());
            build(&user, &mut builder)?;
            match builder.commit(&self.0).await {
                Ok(_) => return Ok(user),
                Err(WriteError::InvalidOriginalVersion) if attempt < USER_APPEND_ATTEMPTS => {
                    tracing::debug!(user_id, attempt, "user stream raced, retrying");
                }
                Err(err) => return Err(err.into()),
            }
        }
    }
}
