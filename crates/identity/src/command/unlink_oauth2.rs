use evento::Executor;

use crate::aggregator::OAuth2AccountUnlinked;
use crate::error::IdentityError;

use super::oauth2_index::free_oauth2_index;

#[evento::command]
impl<E: Executor> super::Command<E> {
    /// Removes a provider identity from the user's account. The user keeps at
    /// least one way to sign in. The account is written first: from then on
    /// the identity signs nobody in, whatever its index entry says. The entry
    /// is freed next, and by the index-release subscription should that fail.
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

        if let Err(err) =
            free_oauth2_index(&self.executor, user_id, provider, provider_user_id, None).await
        {
            tracing::warn!(%err, user_id, "the unlinked identity's index entry is freed later");
        }
        Ok(())
    }
}
