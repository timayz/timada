use evento::ProjectionAggregate;
use evento::{Executor, WriteError};

use crate::aggregator::Linked;
use crate::error::IdentityError;
use crate::value_object::{ProviderIdentity, oauth2_account_id};

use super::Command;

impl<E: Executor> Command<E> {
    /// Points the provider identity at `user_id`, creating or re-linking the
    /// index entry. Fails when it already points at someone else.
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
                if existing.unlinked {
                    existing
                        .write()?
                        .event(&linked)
                        .commit(&self.executor)
                        .await?;
                    Ok(())
                } else if existing.user_id == user_id {
                    Ok(())
                } else {
                    Err(IdentityError::AlreadyLinkedToOtherUser)
                }
            }
            Err(err) => Err(err.into()),
        }
    }
}
