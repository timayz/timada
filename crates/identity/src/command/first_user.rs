//! The first registered user administers the instance (oauth2-passkey's
//! "sequence number 1" rule). A singleton stream makes the claim atomic.

use evento::{Executor, WriteError};

use crate::aggregator::{AdminGranted, FirstUserClaimed};
use crate::error::IdentityError;
use crate::value_object::REGISTRY_ID;

use super::{Command, UserState};

impl<E: Executor> Command<E> {
    /// Tries to make `user_id` the first user. Exactly one caller ever wins;
    /// the winner is granted admin. Returns whether this call won.
    pub(crate) async fn claim_first_user(
        &self,
        user_id: &str,
        routing_key: Option<String>,
    ) -> Result<bool, IdentityError> {
        let claimed = evento::append(REGISTRY_ID)
            .original_version(0)
            .routing_key_opt(routing_key.clone())
            .event(&FirstUserClaimed {
                user_id: user_id.to_owned(),
            })
            .commit(&self.0)
            .await;
        match claimed {
            Ok(_) => {
                self.grant_admin(user_id, routing_key).await?;
                Ok(true)
            }
            Err(WriteError::InvalidOriginalVersion) => {
                // Someone else is first. Unless the claim already names this
                // user and its grant never landed: repair it now.
                if let Some(user) = self.load_user(user_id).await? {
                    self.grant_admin_if_first(&user, routing_key).await?;
                }
                Ok(false)
            }
            Err(err) => Err(err.into()),
        }
    }

    /// Repairs a claim whose admin grant never landed (a crash between the two
    /// writes). Called on every sign-in; a no-op for everyone else.
    pub(crate) async fn grant_admin_if_first(
        &self,
        user: &UserState,
        routing_key: Option<String>,
    ) -> Result<(), IdentityError> {
        if user.is_admin {
            return Ok(());
        }
        let Some(registry) = self.load_registry().await? else {
            return Ok(());
        };
        if registry.first_user_id != user.id {
            return Ok(());
        }
        tracing::warn!(user_id = %user.id, "first user was missing the admin grant, repairing");
        self.grant_admin(&user.id, routing_key).await
    }

    async fn grant_admin(
        &self,
        user_id: &str,
        routing_key: Option<String>,
    ) -> Result<(), IdentityError> {
        self.append_user(user_id, routing_key, |user, builder| {
            if !user.is_admin {
                builder.event(&AdminGranted);
            } else {
                return Err(IdentityError::InvalidState);
            }
            Ok(())
        })
        .await
        .map(|_| ())
        .or_else(|err| match err {
            // Already admin: nothing to do.
            IdentityError::InvalidState => Ok(()),
            other => Err(other),
        })
    }
}
