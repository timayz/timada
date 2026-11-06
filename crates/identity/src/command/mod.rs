//! Write side: the command gateway and the strict write models it loads.
//!
//! Every write model folds *all* events of its aggregate (`strict()`), so the
//! version it observes, and `write()` sends as `original_version`, never goes
//! stale. None of them is snapshotted: streams are a handful of events.

mod add_email;
mod add_passkey;
mod admin;
mod change_password;
mod check_password;
mod complete_oauth2;
mod email_challenge;
mod email_index;
mod end_session;
mod first_user;
mod oauth2_index;
mod passkey_index;
mod refresh_session;
mod register_with_password;
mod remove_passkey;
mod rename_passkey;
mod reset_password;
mod resolve_session;
mod revoke_sessions;
mod sign_in;
mod sign_in_passkey;
mod sign_in_with_password;
mod start_session;
mod unlink_oauth2;
mod update_profile;

use std::ops::Deref;
use std::sync::Arc;

pub use add_email::*;
pub use add_passkey::*;
pub use change_password::*;
pub use check_password::*;
pub use complete_oauth2::*;
pub use email_challenge::*;
pub use register_with_password::*;
pub use reset_password::*;
pub use resolve_session::*;
pub use sign_in_passkey::*;
pub use sign_in_with_password::*;

pub(crate) use admin::free_indexes;
pub(crate) use oauth2_index::free_oauth2_index;
pub(crate) use passkey_index::free_passkey_index;

use evento::{
    Executor, Projection, ProjectionAggregate, WriteBuilder, WriteError, metadata::Event,
};
use timada_core::CodeVerifier;

use crate::aggregator::{
    AdminGranted, AdminRevoked, AttemptFailed, Claimed, CodeIssued, Consumed, Created, Deleted,
    EmailAdded, EmailRegistered, Ended, FirstUserClaimed, Linked, OAuth2AccountLinked,
    OAuth2AccountUnlinked, PasskeyAdded, PasskeyRemoved, PasskeyRenamed, PasswordChanged,
    ProfileUpdated, Refreshed, Registered, Released, Removed, SessionsRevoked, SignedIn,
    SignedInWithPassword, SignedOut, Started, Unlinked, Updated,
};
use crate::error::IdentityError;
use crate::value_object::{
    ChallengePurpose, REGISTRY_ID, SignInKind, email_account_id, email_challenge_id,
    oauth2_account_id, passkey_credential_id, session_id,
};

/// The command gateway: the executor, and what checks the mailed codes
/// against what their streams keep (a key the host holds, see
/// [`CodeVerifier`]).
pub struct Command<E: Executor> {
    executor: E,
    codes: Arc<dyn CodeVerifier>,
}

impl<E: Executor> Command<E> {
    pub fn new(executor: E, codes: Arc<dyn CodeVerifier>) -> Self {
        Self { executor, codes }
    }
}

impl<E: Executor> Deref for Command<E> {
    type Target = E;

    fn deref(&self) -> &Self::Target {
        &self.executor
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

/// The e-mail + password sign-in method of a user.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PasswordMethod {
    /// Normalized address, also the user's login identifier.
    pub email: String,
    /// PHC string.
    pub password_hash: String,
    /// Unix seconds of the latest `EmailRegistered` / `EmailAdded` /
    /// `PasswordChanged`.
    pub changed_at: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SignInRecord {
    pub method: SignInKind,
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

    /// Whether the session was signed in before this revocation and not
    /// spared by it: ended by it for sure. One started within the
    /// revocation's own millisecond is not counted: stamps are milliseconds,
    /// and it may be the session the asking browser signed in right after.
    pub fn ended_before(&self, session: &SessionState) -> bool {
        self.covers(session) && session.started_at_ms < self.at_ms
    }
}

/// Unix milliseconds of an event, for cutoffs where a second is too coarse.
pub(crate) fn event_millis(event: &evento::Event) -> u64 {
    event.timestamp * 1000 + u64::from(event.timestamp_subsec)
}

/// How long a claim on an index entry counts as held while the account it
/// points at does not list it yet: the add that made it may be committing
/// right now. Past this, a node stopped in between and the entry is anyone's.
/// The claims of the other contexts give their holders the same five minutes.
const CLAIM_GRACE_SECS: u64 = 300;

/// Whether a claim stamped at `claimed_at_ms` may still be committing at
/// `now` (unix seconds).
fn claim_is_fresh(claimed_at_ms: u64, now: u64) -> bool {
    now.saturating_sub(claimed_at_ms / 1000) < CLAIM_GRACE_SECS
}

/// What a claim does with an index entry that already exists.
pub(crate) enum Claiming {
    /// Write the claim: the entry is free, the claimant's own but not listed
    /// (claimed afresh, so the release of an older removal leaves it), or
    /// abandoned.
    Take,
    /// Nothing to write: the claimant's account lists it already.
    Keep,
    /// Another account lists it, or may be adding it right now.
    Refuse,
}

/// Whether a user write that failed surely did not land, so what the command
/// claimed for it may be given back. One the store could not settle
/// (`Unknown`) or would not take right now (`Unavailable`) may still commit:
/// its claim is kept, held for the grace, and anyone's after it should the
/// account never list it.
pub(crate) fn surely_not_written(err: &IdentityError) -> bool {
    !matches!(
        err,
        IdentityError::Write(WriteError::Unknown(_) | WriteError::Unavailable(_))
    )
}

/// How far back a command looks in a user's stream for the event it has just
/// appended.
const APPENDED_LOOKBACK: u16 = 8;

#[evento::projection(id = id, name = "timada-identity/UserState")]
#[evento::snapshot(none)]
#[derive(Debug, PartialEq, Eq)]
pub struct UserState {
    pub id: String,
    pub account: String,
    pub label: String,
    pub is_admin: bool,
    /// An administrator decided the role (granted or revoked). Until then the
    /// first-user rule may still grant it.
    pub role_assigned: bool,
    pub oauth2_accounts: Vec<LinkedOAuth2Account>,
    pub passkeys: Vec<LinkedPasskey>,
    pub password: Option<PasswordMethod>,
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
        self.oauth2_accounts.len() + self.passkeys.len() + usize::from(self.password.is_some())
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

    /// Whether the e-mail + password method is for this normalized address.
    pub fn has_email(&self, email: &str) -> bool {
        self.password.as_ref().is_some_and(|p| p.email == email)
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
        .handler(on_admin_revoked())
        .handler(on_oauth2_account_linked())
        .handler(on_oauth2_account_unlinked())
        .handler(on_passkey_added())
        .handler(on_passkey_renamed())
        .handler(on_passkey_removed())
        .handler(on_signed_in())
        .handler(on_deleted())
        .handler(on_sessions_revoked())
        .handler(on_email_registered())
        .handler(on_password_changed())
        .handler(on_signed_in_with_password())
        .handler(on_email_added())
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
    user.role_assigned = true;
    Ok(())
}

#[evento::handler]
async fn on_admin_revoked(_event: Event<AdminRevoked>, user: &mut UserState) -> anyhow::Result<()> {
    user.is_admin = false;
    user.role_assigned = true;
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
    push_sign_in(
        user,
        SignInRecord {
            method: event.data.method.into(),
            provider: event.data.provider.to_owned(),
            credential_id: event.data.credential_id.to_owned(),
            user_agent: event.data.user_agent.to_owned(),
            ip: event.data.ip.to_owned(),
            at: event.timestamp,
        },
    );
    Ok(())
}

fn push_sign_in(user: &mut UserState, record: SignInRecord) {
    user.sign_ins.push(record);
    if user.sign_ins.len() > SIGN_IN_HISTORY {
        let excess = user.sign_ins.len() - SIGN_IN_HISTORY;
        user.sign_ins.drain(..excess);
    }
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

#[evento::handler]
async fn on_email_registered(
    event: Event<EmailRegistered>,
    user: &mut UserState,
) -> anyhow::Result<()> {
    user.id = event.aggregate_id.to_owned();
    user.account = event.data.email.to_owned();
    user.label = event.data.label.to_owned();
    user.registered_at = event.timestamp;
    user.password = Some(PasswordMethod {
        email: event.data.email.to_owned(),
        password_hash: event.data.password_hash.to_owned(),
        changed_at: event.timestamp,
    });
    Ok(())
}

#[evento::handler]
async fn on_password_changed(
    event: Event<PasswordChanged>,
    user: &mut UserState,
) -> anyhow::Result<()> {
    if let Some(password) = user.password.as_mut() {
        password.password_hash = event.data.password_hash.to_owned();
        password.changed_at = event.timestamp;
    }
    Ok(())
}

#[evento::handler]
async fn on_signed_in_with_password(
    event: Event<SignedInWithPassword>,
    user: &mut UserState,
) -> anyhow::Result<()> {
    push_sign_in(
        user,
        SignInRecord {
            method: SignInKind::Password,
            provider: None,
            credential_id: None,
            user_agent: event.data.user_agent.to_owned(),
            ip: event.data.ip.to_owned(),
            at: event.timestamp,
        },
    );
    Ok(())
}

#[evento::handler]
async fn on_email_added(event: Event<EmailAdded>, user: &mut UserState) -> anyhow::Result<()> {
    user.password = Some(PasswordMethod {
        email: event.data.email.to_owned(),
        password_hash: event.data.password_hash.to_owned(),
        changed_at: event.timestamp,
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
    /// Unix milliseconds of the latest `Linked`.
    pub claimed_at_ms: u64,
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
    state.claimed_at_ms = event_millis(&event);
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
    /// Unix milliseconds of the latest `Created`.
    pub claimed_at_ms: u64,
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
    state.claimed_at_ms = event_millis(&event);
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
// EmailAccount index
// =============================================================================

#[evento::projection(id = id, name = "timada-identity/EmailAccountState")]
#[evento::snapshot(none)]
#[derive(Debug, PartialEq, Eq)]
pub struct EmailAccountState {
    pub id: String,
    pub user_id: String,
    pub email: String,
    pub released: bool,
    /// Unix milliseconds of the latest `Claimed`.
    pub claimed_at_ms: u64,
}

pub fn email_account_projection<E: Executor>() -> Projection<E, EmailAccountState> {
    Projection::new::<crate::aggregator::EmailAccount>()
        .handler(on_email_claimed())
        .handler(on_email_released())
        .strict()
}

#[evento::handler]
async fn on_email_claimed(
    event: Event<Claimed>,
    state: &mut EmailAccountState,
) -> anyhow::Result<()> {
    state.id = event.aggregate_id.to_owned();
    state.user_id = event.data.user_id.to_owned();
    state.email = event.data.email.to_owned();
    state.released = false;
    state.claimed_at_ms = event_millis(&event);
    Ok(())
}

#[evento::handler]
async fn on_email_released(
    _event: Event<Released>,
    state: &mut EmailAccountState,
) -> anyhow::Result<()> {
    state.released = true;
    Ok(())
}

// =============================================================================
// EmailChallenge
// =============================================================================

/// The latest code mailed to an address and what happened to it.
#[evento::projection(id = id, name = "timada-identity/EmailChallengeState")]
#[evento::snapshot(none)]
#[derive(Debug, PartialEq, Eq)]
pub struct EmailChallengeState {
    pub id: String,
    pub purpose: ChallengePurpose,
    /// The code's MAC under the host's key; see `CodeIssued`.
    pub code_tag: Vec<u8>,
    /// Unix seconds.
    pub issued_at: u64,
    /// Unix seconds.
    pub expires_at: u64,
    /// Wrong guesses against the current code.
    pub failed_attempts: u32,
    /// The current code was used.
    pub consumed: bool,
}

impl EmailChallengeState {
    /// Whether the current code may still be tried at `now`.
    pub fn is_open_at(&self, now: u64) -> bool {
        !self.consumed
            && self.failed_attempts < crate::value_object::CODE_MAX_ATTEMPTS
            && now < self.expires_at
    }
}

pub fn email_challenge_projection<E: Executor>() -> Projection<E, EmailChallengeState> {
    Projection::new::<crate::aggregator::EmailChallenge>()
        .handler(on_code_issued())
        .handler(on_attempt_failed())
        .handler(on_consumed())
        .strict()
}

#[evento::handler]
async fn on_code_issued(
    event: Event<CodeIssued>,
    state: &mut EmailChallengeState,
) -> anyhow::Result<()> {
    state.id = event.aggregate_id.to_owned();
    state.purpose = event.data.purpose;
    state.code_tag = event.data.code_tag.to_owned();
    state.issued_at = event.data.issued_at;
    state.expires_at = event.data.expires_at;
    state.failed_attempts = 0;
    state.consumed = false;
    Ok(())
}

#[evento::handler]
async fn on_attempt_failed(
    _event: Event<AttemptFailed>,
    state: &mut EmailChallengeState,
) -> anyhow::Result<()> {
    state.failed_attempts += 1;
    Ok(())
}

#[evento::handler]
async fn on_consumed(
    _event: Event<Consumed>,
    state: &mut EmailChallengeState,
) -> anyhow::Result<()> {
    state.consumed = true;
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
        Ok(self.load_user_any(id).await?.filter(|u| !u.deleted))
    }

    /// The user including a deleted one (administration pages show those).
    pub async fn load_user_any(&self, id: impl Into<String>) -> anyhow::Result<Option<UserState>> {
        user_projection().load(id).execute(&self.executor).await
    }

    /// The index entry a provider identity signs in through: linked, and
    /// listed on the account it points at.
    pub async fn load_oauth2_account(
        &self,
        provider: &str,
        provider_user_id: &str,
    ) -> anyhow::Result<Option<OAuth2AccountState>> {
        let Some(entry) = self
            .load_oauth2_account_any(provider, provider_user_id)
            .await?
        else {
            return Ok(None);
        };
        Ok(self.oauth2_holder(&entry).await?.map(|_| entry))
    }

    /// The live user an entry points at, if their account lists the provider
    /// identity. An entry unlinked, or left behind by an account that let go
    /// of the identity (unlinked or deleted, the entry not freed yet) or never
    /// recorded it (a crash in between), holds nobody.
    pub(crate) async fn oauth2_holder(
        &self,
        entry: &OAuth2AccountState,
    ) -> anyhow::Result<Option<UserState>> {
        if entry.unlinked {
            return Ok(None);
        }
        Ok(self
            .load_user(&entry.user_id)
            .await?
            .filter(|user| user.has_oauth2_account(&entry.provider, &entry.provider_user_id)))
    }

    /// The index entry including unlinked ones (needed to re-link).
    pub(crate) async fn load_oauth2_account_any(
        &self,
        provider: &str,
        provider_user_id: &str,
    ) -> anyhow::Result<Option<OAuth2AccountState>> {
        oauth2_account_projection()
            .load(oauth2_account_id(provider, provider_user_id))
            .execute(&self.executor)
            .await
    }

    /// The registered passkey for a base64url credential id: its index
    /// entry, while the account it points at lists it.
    pub async fn load_passkey(
        &self,
        credential_id: &str,
    ) -> anyhow::Result<Option<PasskeyCredentialState>> {
        let Some(entry) = self.load_passkey_any(credential_id).await? else {
            return Ok(None);
        };
        Ok(self.passkey_holder(&entry).await?.map(|_| entry))
    }

    /// The live user an entry points at, if their account lists the passkey;
    /// see [`Self::oauth2_holder`].
    pub(crate) async fn passkey_holder(
        &self,
        entry: &PasskeyCredentialState,
    ) -> anyhow::Result<Option<UserState>> {
        if entry.removed {
            return Ok(None);
        }
        Ok(self
            .load_user(&entry.user_id)
            .await?
            .filter(|user| user.passkey(&entry.credential_id).is_some()))
    }

    pub(crate) async fn load_passkey_any(
        &self,
        credential_id: &str,
    ) -> anyhow::Result<Option<PasskeyCredentialState>> {
        passkey_credential_projection()
            .load(passkey_credential_id(credential_id))
            .execute(&self.executor)
            .await
    }

    /// The index entry a normalized address signs in through: claimed, and
    /// the address of the e-mail + password method of the account it points
    /// at.
    pub async fn load_email_account(
        &self,
        email: &str,
    ) -> anyhow::Result<Option<EmailAccountState>> {
        let Some(entry) = self.load_email_account_any(email).await? else {
            return Ok(None);
        };
        Ok(self.email_holder(&entry).await?.map(|_| entry))
    }

    /// The live user a normalized address signs in as, through
    /// [`Self::load_email_account`]'s entry.
    pub(crate) async fn email_user(&self, email: &str) -> anyhow::Result<Option<UserState>> {
        let Some(entry) = self.load_email_account_any(email).await? else {
            return Ok(None);
        };
        self.email_holder(&entry).await
    }

    /// The live user an entry points at, if the address is the one of their
    /// e-mail + password method; see [`Self::oauth2_holder`].
    pub(crate) async fn email_holder(
        &self,
        entry: &EmailAccountState,
    ) -> anyhow::Result<Option<UserState>> {
        if entry.released {
            return Ok(None);
        }
        Ok(self
            .load_user(&entry.user_id)
            .await?
            .filter(|user| user.has_email(&entry.email)))
    }

    /// What a claim by `claimant_id` does with an existing entry that is
    /// `freed` or points at `owner_id`, claimed at `claimed_at_ms`; `lists`
    /// tells whether an account lists what the entry indexes.
    pub(crate) async fn claiming(
        &self,
        freed: bool,
        owner_id: &str,
        claimed_at_ms: u64,
        claimant_id: &str,
        lists: impl Fn(&UserState) -> bool,
    ) -> anyhow::Result<Claiming> {
        if freed {
            return Ok(Claiming::Take);
        }
        let owner = self.load_user(owner_id).await?;
        let listed = owner.as_ref().is_some_and(&lists);
        Ok(if owner_id == claimant_id {
            if listed {
                Claiming::Keep
            } else {
                Claiming::Take
            }
        } else if listed
            || (owner.is_some() && claim_is_fresh(claimed_at_ms, timada_core::unix_now()))
        {
            Claiming::Refuse
        } else {
            Claiming::Take
        })
    }

    /// When the event a command appended onto `user` (the state
    /// [`Self::append_user`] returned) was stamped, in unix milliseconds:
    /// what the release that follows compares claims with. `None` once later
    /// writes buried it.
    pub(crate) async fn appended_at_ms(&self, user: &UserState) -> anyhow::Result<Option<u64>> {
        let Some(version) = user.aggregate_version()?.checked_add(1) else {
            return Ok(None);
        };
        let tail = evento::read::<crate::aggregator::User>(&user.id)
            .backward()
            .limit(APPENDED_LOOKBACK)
            .execute(&self.executor)
            .await?;
        Ok(tail
            .iter()
            .find(|event| event.version == version)
            .map(event_millis))
    }

    /// The index entry including released ones (needed to re-claim).
    pub(crate) async fn load_email_account_any(
        &self,
        email: &str,
    ) -> anyhow::Result<Option<EmailAccountState>> {
        email_account_projection()
            .load(email_account_id(email))
            .execute(&self.executor)
            .await
    }

    /// The latest code mailed to a normalized address, if any was.
    pub async fn load_email_challenge(
        &self,
        email: &str,
    ) -> anyhow::Result<Option<EmailChallengeState>> {
        email_challenge_projection()
            .load(email_challenge_id(email))
            .execute(&self.executor)
            .await
    }

    pub async fn load_session(
        &self,
        token_hash: &[u8; 32],
    ) -> anyhow::Result<Option<SessionState>> {
        session_projection()
            .load(session_id(token_hash))
            .execute(&self.executor)
            .await
    }

    pub async fn load_registry(&self) -> anyhow::Result<Option<RegistryState>> {
        registry_projection()
            .load(REGISTRY_ID)
            .execute(&self.executor)
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
            match builder.commit(&self.executor).await {
                Ok(_) => return Ok(user),
                Err(WriteError::InvalidOriginalVersion) if attempt < USER_APPEND_ATTEMPTS => {
                    tracing::debug!(user_id, attempt, "user stream raced, retrying");
                }
                Err(err) => return Err(err.into()),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_claim_stays_fresh_for_the_grace_only() {
        let claimed_at_ms = 1_000_000;
        assert!(claim_is_fresh(claimed_at_ms, 1_000));
        assert!(claim_is_fresh(claimed_at_ms, 1_000 + CLAIM_GRACE_SECS - 1));
        assert!(!claim_is_fresh(claimed_at_ms, 1_000 + CLAIM_GRACE_SECS));
        // A clock behind the claim's stamp keeps it fresh.
        assert!(claim_is_fresh(claimed_at_ms, 900));
    }

    #[test]
    fn a_revocation_ends_the_sessions_started_before_its_millisecond() {
        let revocation = SessionsRevocation {
            at_ms: 5_000,
            except_session: Some("spared".to_owned()),
        };
        let session = |id: &str, started_at_ms| SessionState {
            id: id.to_owned(),
            started_at_ms,
            ..SessionState::default()
        };
        assert!(revocation.ended_before(&session("old", 4_999)));
        assert!(
            !revocation.ended_before(&session("spared", 4_999)),
            "the one it spared"
        );
        assert!(revocation.covers(&session("tie", 5_000)));
        assert!(
            !revocation.ended_before(&session("tie", 5_000)),
            "a tie may be the asking browser's new session"
        );
        assert!(!revocation.ended_before(&session("new", 5_001)));
    }
}
