use evento::{Executor, WriteError};

use crate::aggregator::{PasskeyAdded, Registered, SignedIn};
use crate::error::IdentityError;
use crate::value_object::{LoginContext, MAX_PASSKEYS_PER_USER, RegistrationMode, SignInMethod};

pub use super::passkey_index::VerifiedPasskey;

/// A finished registration ceremony.
#[derive(Debug, Clone)]
pub struct RegisterPasskey {
    pub mode: RegistrationMode,
    /// The user the browser is signed in as when the ceremony finishes.
    pub session_user_id: Option<String>,
    /// The user id the ceremony was started for: the signed-in user for
    /// `AddToUser`, a freshly generated id for `CreateUser`. Carried through
    /// the flow cookie, so a session change mid-ceremony is detected.
    pub bound_user_id: String,
    pub passkey: VerifiedPasskey,
    /// Login identifier and display name for a new user.
    pub account: String,
    pub label: String,
    pub context: LoginContext,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PasskeyOutcome {
    pub user_id: String,
    /// A new user was registered (and signed in).
    pub created: bool,
}

#[evento::command]
impl<E: Executor> super::Command<E> {
    pub async fn register_passkey(
        &self,
        cmd: RegisterPasskey,
        routing_key: Option<String>,
    ) -> Result<PasskeyOutcome, IdentityError> {
        match cmd.mode {
            RegistrationMode::AddToUser => {
                let Some(session_user_id) = cmd.session_user_id.as_deref() else {
                    return Err(IdentityError::Unauthorized);
                };
                if session_user_id != cmd.bound_user_id {
                    return Err(IdentityError::SessionMismatch);
                }
                self.add_passkey(&cmd.bound_user_id, &cmd.passkey, routing_key)
                    .await?;
                Ok(PasskeyOutcome {
                    user_id: cmd.bound_user_id,
                    created: false,
                })
            }
            RegistrationMode::CreateUser => {
                if cmd.session_user_id.is_some() {
                    return Err(IdentityError::UnexpectedlyAuthorized);
                }
                if self
                    .load_passkey(&cmd.passkey.credential_id)
                    .await?
                    .is_some()
                {
                    return Err(IdentityError::AlreadyRegistered);
                }
                self.create_user_with_passkey(&cmd, routing_key).await?;
                Ok(PasskeyOutcome {
                    user_id: cmd.bound_user_id,
                    created: true,
                })
            }
        }
    }
}

impl<E: Executor> super::Command<E> {
    async fn add_passkey(
        &self,
        user_id: &str,
        passkey: &VerifiedPasskey,
        routing_key: Option<String>,
    ) -> Result<(), IdentityError> {
        let Some(user) = self.load_user(user_id).await? else {
            return Err(IdentityError::UserNotFound);
        };
        if user.passkeys.len() >= MAX_PASSKEYS_PER_USER {
            return Err(IdentityError::TooManyPasskeys);
        }
        self.create_passkey_index(passkey, user_id, routing_key.clone())
            .await?;
        let result = self
            .append_user(user_id, routing_key, |user, builder| {
                if user.passkey(&passkey.credential_id).is_some() {
                    return Err(IdentityError::InvalidState);
                }
                if user.passkeys.len() >= MAX_PASSKEYS_PER_USER {
                    return Err(IdentityError::TooManyPasskeys);
                }
                builder.event(&PasskeyAdded {
                    credential_id: passkey.credential_id.clone(),
                    name: passkey.name.clone(),
                    aaguid: passkey.aaguid.clone(),
                });
                Ok(())
            })
            .await;
        match result {
            Ok(_) | Err(IdentityError::InvalidState) => Ok(()),
            Err(err) => Err(err),
        }
    }

    /// The user stream first (a stub nobody can reach if the index race is
    /// lost), then the index entry, then the first-user claim.
    async fn create_user_with_passkey(
        &self,
        cmd: &RegisterPasskey,
        routing_key: Option<String>,
    ) -> Result<(), IdentityError> {
        let user_id = cmd.bound_user_id.as_str();
        let created = evento::append(user_id)
            .original_version(0)
            .routing_key_opt(routing_key.clone())
            .event(&Registered {
                account: cmd.account.clone(),
                label: cmd.label.clone(),
                method: SignInMethod::Passkey,
            })
            .event(&PasskeyAdded {
                credential_id: cmd.passkey.credential_id.clone(),
                name: cmd.passkey.name.clone(),
                aaguid: cmd.passkey.aaguid.clone(),
            })
            .event(&SignedIn {
                method: SignInMethod::Passkey,
                provider: None,
                credential_id: Some(cmd.passkey.credential_id.clone()),
                user_agent: cmd.context.user_agent.clone(),
                ip: cmd.context.ip.clone(),
            })
            .commit(&self.0)
            .await;
        match created {
            Ok(_) => {}
            // The bound id was already used: a replayed finish.
            Err(WriteError::InvalidOriginalVersion) => return Err(IdentityError::InvalidState),
            Err(err) => return Err(err.into()),
        }

        match self
            .create_passkey_index(&cmd.passkey, user_id, routing_key.clone())
            .await
        {
            Ok(()) => {}
            Err(IdentityError::AlreadyRegistered) => {
                tracing::warn!(
                    orphan_user_id = %user_id,
                    "lost the passkey registration race; the stub user is unreachable"
                );
                return Err(IdentityError::AlreadyRegistered);
            }
            Err(err) => return Err(err),
        }

        self.claim_first_user(user_id, routing_key).await?;
        Ok(())
    }
}
