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

        if let Some(index) = self.load_oauth2_account(provider, provider_user_id).await? {
            if index.user_id != user_id {
                return Err(IdentityError::Forbidden);
            }
            index
                .write()?
                .routing_key_opt(routing_key.clone())
                .event(&Unlinked)
                .commit(&self.0)
                .await?;
        }

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
