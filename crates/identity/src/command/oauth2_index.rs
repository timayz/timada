use evento::ProjectionAggregate;
use evento::{Executor, WriteError};

use crate::aggregator::{Linked, Unlinked};
use crate::error::IdentityError;
use crate::value_object::{ProviderIdentity, oauth2_account_id};

use super::{Command, oauth2_account_projection, user_projection};

impl<E: Executor> Command<E> {
    /// Points the provider identity at `user_id`, creating or re-linking the
    /// index entry. Fails when another user holds it; an entry nobody holds
    /// (unlinked, or left behind by an account that let go of it) is taken
    /// over.
    pub(crate) async fn link_oauth2_index(
        &self,
        identity: &ProviderIdentity,
        user_id: &str,
        routing_key: Option<String>,
    ) -> Result<(), IdentityError> {
        let linked = Linked {
            user_id: user_id.to_owned(),
            provider: identity.provider.clone(),
            provider_user_id: identity.provider_user_id.clone(),
        };
        let created = evento::append(oauth2_account_id(
            &identity.provider,
            &identity.provider_user_id,
        ))
        .original_version(0)
        .routing_key_opt(routing_key)
        .event(&linked)
        .commit(&self.executor)
        .await;
        match created {
            Ok(_) => Ok(()),
            Err(WriteError::InvalidOriginalVersion) => {
                let Some(existing) = self
                    .load_oauth2_account_any(&identity.provider, &identity.provider_user_id)
                    .await?
                else {
                    return Err(IdentityError::InvalidState);
                };
                match self.oauth2_holder(&existing).await? {
                    Some(holder) if holder.id == user_id => Ok(()),
                    Some(_) => Err(IdentityError::AlreadyLinkedToOtherUser),
                    None => {
                        existing
                            .write()?
                            .event(&linked)
                            .commit(&self.executor)
                            .await?;
                        Ok(())
                    }
                }
            }
            Err(err) => Err(err.into()),
        }
    }
}

/// Frees the index entry of a provider identity `user_id` let go of
/// (unlinked it, or was deleted), so it can be registered or linked again.
/// Same rules as [`super::passkey_index::free_passkey_index`].
pub(crate) async fn free_oauth2_index<E: Executor>(
    executor: &E,
    user_id: &str,
    provider: &str,
    provider_user_id: &str,
    let_go_at_ms: Option<u64>,
) -> Result<(), IdentityError> {
    let Some(entry) = oauth2_account_projection()
        .load(oauth2_account_id(provider, provider_user_id))
        .execute(executor)
        .await?
    else {
        return Ok(());
    };
    if entry.unlinked
        || entry.user_id != user_id
        || let_go_at_ms.is_some_and(|at| entry.claimed_at_ms >= at)
    {
        return Ok(());
    }
    let user = user_projection().load(user_id).execute(executor).await?;
    if user.is_some_and(|user| !user.deleted && user.has_oauth2_account(provider, provider_user_id))
    {
        return Ok(());
    }
    entry.write()?.event(&Unlinked).commit(executor).await?;
    Ok(())
}
