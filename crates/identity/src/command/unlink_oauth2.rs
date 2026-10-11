use evento::Executor;
use evento::ProjectionAggregate;

use crate::aggregator::{OAuth2AccountUnlinked, Unlinked};
use crate::error::IdentityError;

#[evento::command]
impl<E: Executor> super::Command<E> {
    /// Removes a provider identity from the user's account. The user keeps at
    /// least one way to sign in.
    pub async fn unlink_oauth2(
        &self,
        user_id: &str,
        provider: &str,
        provider_user_id: &str,
        routing_key: Option<String>,
    ) -> Result<(), IdentityError> {
        let Some(user) = self.load_user(user_id).await? else {
            return Err(IdentityError::UserNotFound);
        };
        if !user.has_oauth2_account(provider, provider_user_id) {
            return Err(IdentityError::Forbidden);
        }
        if user.sign_in_methods() <= 1 {
            return Err(IdentityError::LastSignInMethod);
        }

        self.unlink_oauth2_index(user_id, provider, provider_user_id, routing_key.clone())
            .await?;

        self.append_user(user_id, routing_key, |user, builder| {
            if !user.has_oauth2_account(provider, provider_user_id) {
                return Err(IdentityError::Forbidden);
            }
            if user.sign_in_methods() <= 1 {
                return Err(IdentityError::LastSignInMethod);
            }
            builder.event(&OAuth2AccountUnlinked {
                provider: provider.to_owned(),
                provider_user_id: provider_user_id.to_owned(),
            });
            Ok(())
        })
        .await?;
        Ok(())
    }
}

impl<E: Executor> super::Command<E> {
    /// Frees the index entry of a provider identity owned by `user_id`, so it
    /// can be registered or linked again. A missing or already freed entry is
    /// fine; one owned by someone else is `Forbidden`.
    pub(crate) async fn unlink_oauth2_index(
        &self,
        user_id: &str,
        provider: &str,
        provider_user_id: &str,
        routing_key: Option<String>,
    ) -> Result<(), IdentityError> {
        if let Some(index) = self.load_oauth2_account(provider, provider_user_id).await? {
            if index.user_id != user_id {
                return Err(IdentityError::Forbidden);
            }
            index
                .write()?
                .routing_key_opt(routing_key)
                .event(&Unlinked)
                .commit(&self.0)
                .await?;
        }
        Ok(())
    }
}
