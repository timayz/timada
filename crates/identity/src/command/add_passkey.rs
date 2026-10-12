use evento::Executor;

use crate::aggregator::PasskeyAdded;
use crate::error::IdentityError;
use crate::value_object::MAX_PASSKEYS_PER_USER;

pub use super::passkey_index::VerifiedPasskey;

/// A finished registration ceremony for a signed-in user. Passkeys never
/// create accounts: a person registers with a provider or an e-mail address
/// first, then adds passkeys to that account.
#[derive(Debug, Clone)]
pub struct AddPasskey {
    /// The user the browser is signed in as when the ceremony finishes.
    pub session_user_id: Option<String>,
    /// The user the ceremony was started for, carried through the flow
    /// cookie so a session change mid-ceremony is detected.
    pub bound_user_id: String,
    pub passkey: VerifiedPasskey,
}

#[evento::command]
impl<E: Executor> super::Command<E> {
    pub async fn add_passkey(
        &self,
        cmd: AddPasskey,
        routing_key: Option<String>,
    ) -> Result<(), IdentityError> {
        let Some(session_user_id) = cmd.session_user_id.as_deref() else {
            return Err(IdentityError::Unauthorized);
        };
        if session_user_id != cmd.bound_user_id {
            return Err(IdentityError::SessionMismatch);
        }
        self.attach_passkey(&cmd.bound_user_id, &cmd.passkey, routing_key)
            .await
    }
}

impl<E: Executor> super::Command<E> {
    /// Index first (the uniqueness check), then the user's own record of it.
    async fn attach_passkey(
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
}
