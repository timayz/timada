use evento::ProjectionAggregate;
use evento::{Executor, WriteError};

use crate::aggregator::{Claimed, Released};
use crate::error::IdentityError;
use crate::value_object::email_account_id;

use super::Command;

impl<E: Executor> Command<E> {
    /// Points a normalized address at `user_id`, creating or re-claiming the
    /// index entry. Fails when it already points at someone else.
    pub(crate) async fn claim_email_index(
        &self,
        email: &str,
        user_id: &str,
        routing_key: Option<String>,
    ) -> Result<(), IdentityError> {
        let claimed = Claimed {
            user_id: user_id.to_owned(),
            email: email.to_owned(),
        };
        let created = evento::append(email_account_id(email))
            .original_version(0)
            .routing_key_opt(routing_key)
            .event(&claimed)
            .commit(&self.executor)
            .await;
        match created {
            Ok(_) => Ok(()),
            Err(WriteError::InvalidOriginalVersion) => {
                let Some(existing) = self.load_email_account_any(email).await? else {
                    return Err(IdentityError::InvalidState);
                };
                if existing.released {
                    existing
                        .write()?
                        .event(&claimed)
                        .commit(&self.executor)
                        .await?;
                    Ok(())
                } else if existing.user_id == user_id {
                    Ok(())
                } else {
                    Err(IdentityError::AlreadyRegistered)
                }
            }
            Err(err) => Err(err.into()),
        }
    }

    /// Frees the index entry of an address owned by `user_id`, so it can be
    /// registered again. A missing or already freed entry is fine; one owned
    /// by someone else is `Forbidden`.
    pub(crate) async fn release_email_index(
        &self,
        user_id: &str,
        email: &str,
        routing_key: Option<String>,
    ) -> Result<(), IdentityError> {
        if let Some(index) = self.load_email_account(email).await? {
            if index.user_id != user_id {
                return Err(IdentityError::Forbidden);
            }
            index
                .write()?
                .routing_key_opt(routing_key)
                .event(&Released)
                .commit(&self.executor)
                .await?;
        }
        Ok(())
    }
}
