use evento::Executor;

use crate::aggregator::PasskeyRemoved;
use crate::error::IdentityError;

use super::passkey_index::free_passkey_index;

#[evento::command]
impl<E: Executor> super::Command<E> {
    /// Removes a passkey from the user's account. The user keeps at least one
    /// way to sign in. The account is written first: from then on the passkey
    /// signs nobody in, whatever its index entry says. The entry is freed
    /// next, and by the index-release subscription should that fail.
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

        let before = self
            .append_user(user_id, routing_key, |user, builder| {
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

        // Only a claim older than the removal is freed: one made since is an
        // add of the same key, on its way to the account.
        let freed = match self.appended_at_ms(&before).await {
            Ok(Some(removed_at)) => {
                free_passkey_index(&self.executor, user_id, credential_id, Some(removed_at)).await
            }
            Ok(None) => Ok(()),
            Err(err) => Err(err.into()),
        };
        if let Err(err) = freed {
            tracing::warn!(%err, user_id, "the removed passkey's index entry is freed later");
        }
        Ok(())
    }
}
