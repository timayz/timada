use evento::Executor;
use evento::ProjectionAggregate;

use crate::aggregator::Updated;
use crate::error::IdentityError;
use crate::value_object::{LoginContext, SignInKind};

use super::sign_in::SignInFacts;

/// A verified passkey assertion.
#[derive(Debug, Clone)]
pub struct SignInPasskey {
    /// Base64url credential id of the key that signed.
    pub credential_id: String,
    /// The credential re-serialized after the ceremony when its counter or
    /// backup flags changed.
    pub updated_passkey_json: Option<String>,
    /// The user the browser is signed in as, if any. Signing in with another
    /// user's passkey switches accounts.
    pub session_user_id: Option<String>,
    pub context: LoginContext,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PasskeySignIn {
    pub user_id: String,
    /// Display name of that user, for the "signed in as" notice.
    pub label: String,
    /// The browser was signed in as this other user; the web layer replaces
    /// that session.
    pub switched_from: Option<String>,
}

#[evento::command]
impl<E: Executor> super::Command<E> {
    /// Records a verified assertion. Returns the user to start a session for.
    pub async fn sign_in_passkey(
        &self,
        cmd: SignInPasskey,
        routing_key: Option<String>,
    ) -> Result<PasskeySignIn, IdentityError> {
        let Some(credential) = self.load_passkey(&cmd.credential_id).await? else {
            return Err(IdentityError::UnknownCredential);
        };
        if self.load_user(&credential.user_id).await?.is_none() {
            return Err(IdentityError::UserNotFound);
        }
        if let Some(passkey_json) = cmd.updated_passkey_json {
            credential
                .write()?
                .routing_key_opt(routing_key.clone())
                .event(&Updated { passkey_json })
                .commit(&self.0)
                .await?;
        }
        let user_id = credential.user_id;
        let user = self
            .record_sign_in(
                &user_id,
                SignInFacts {
                    kind: SignInKind::Passkey,
                    provider: None,
                    credential_id: Some(cmd.credential_id),
                    context: cmd.context,
                },
                routing_key,
            )
            .await?;
        Ok(PasskeySignIn {
            switched_from: cmd.session_user_id.filter(|me| *me != user_id),
            user_id,
            label: user.label,
        })
    }
}
