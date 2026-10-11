use evento::Executor;
use evento::ProjectionAggregate;

use crate::aggregator::{PasskeyRemoved, Removed};
use crate::error::IdentityError;

#[evento::command]
impl<E: Executor> super::Command<E> {
    pub async fn remove_passkey(
        &self,
        user_id: &str,
        credential_id: &str,
        routing_key: Option<String>,
    ) -> Result<(), IdentityError> {
        let Some(user) = self.load_user(user_id).await? else {
            return Err(IdentityError::UserNotFound);
        };
        if user.passkey(credential_id).is_none() {
            return Err(IdentityError::Forbidden);
        }
        if user.sign_in_methods() <= 1 {
            return Err(IdentityError::LastSignInMethod);
        }

        self.remove_passkey_index(user_id, credential_id, routing_key.clone())
            .await?;

        self.append_user(user_id, routing_key, |user, builder| {
            if user.passkey(credential_id).is_none() {
                return Err(IdentityError::Forbidden);
            }
            if user.sign_in_methods() <= 1 {
                return Err(IdentityError::LastSignInMethod);
            }
            builder.event(&PasskeyRemoved {
                credential_id: credential_id.to_owned(),
            });
            Ok(())
        })
        .await?;
        Ok(())
    }
}

impl<E: Executor> super::Command<E> {
    /// Frees the index entry of a passkey owned by `user_id`, so the
    /// credential can be registered again. A missing or already freed entry
    /// is fine; one owned by someone else is `Forbidden`.
    pub(crate) async fn remove_passkey_index(
        &self,
        user_id: &str,
        credential_id: &str,
        routing_key: Option<String>,
    ) -> Result<(), IdentityError> {
        if let Some(index) = self.load_passkey(credential_id).await? {
            if index.user_id != user_id {
                return Err(IdentityError::Forbidden);
            }
            index
                .write()?
                .routing_key_opt(routing_key)
                .event(&Removed)
                .commit(&self.0)
                .await?;
        }
        Ok(())
    }
}
