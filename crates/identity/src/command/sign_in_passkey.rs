use evento::Executor;
use evento::ProjectionAggregate;

use crate::aggregator::Updated;
use crate::error::IdentityError;
use crate::value_object::{LoginContext, SignInMethod};

use super::sign_in::SignInFacts;

#[evento::command]
impl<E: Executor> super::Command<E> {
    /// Records a verified assertion. `updated_passkey_json` is the credential
    /// re-serialized after the ceremony when its counter or backup flags
    /// changed. Returns the user id to start a session for.
    pub async fn sign_in_passkey(
        &self,
        credential_id: &str,
        updated_passkey_json: Option<String>,
        context: LoginContext,
        routing_key: Option<String>,
    ) -> Result<String, IdentityError> {
        let Some(credential) = self.load_passkey(credential_id).await? else {
            return Err(IdentityError::UnknownCredential);
        };
        if self.load_user(&credential.user_id).await?.is_none() {
            return Err(IdentityError::UserNotFound);
        }
        if let Some(passkey_json) = updated_passkey_json {
            credential
                .write()?
                .routing_key_opt(routing_key.clone())
                .event(&Updated { passkey_json })
                .commit(&self.0)
                .await?;
        }
        let user_id = credential.user_id;
        self.record_sign_in(
            &user_id,
            SignInFacts {
                method: SignInMethod::Passkey,
                provider: None,
                credential_id: Some(credential_id.to_owned()),
                context,
            },
            routing_key,
        )
        .await?;
        Ok(user_id)
    }
}
