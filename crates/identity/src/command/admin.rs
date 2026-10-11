//! Administration: roles and account deletion. Every command checks that the
//! actor administers the instance; the first registered user does by default.

use evento::Executor;

use crate::aggregator::{AdminRevoked, Deleted, SessionsRevoked};
use crate::error::IdentityError;

use super::UserState;

#[evento::command]
impl<E: Executor> super::Command<E> {
    /// Grants or revokes the administrator role. An administrator cannot
    /// revoke their own role, so the instance always keeps one. Unchanged
    /// roles are a no-op.
    pub async fn set_admin(
        &self,
        actor_id: &str,
        user_id: &str,
        admin: bool,
        routing_key: Option<String>,
    ) -> Result<(), IdentityError> {
        self.require_admin(actor_id).await?;
        if actor_id == user_id && !admin {
            return Err(IdentityError::Forbidden);
        }
        self.append_user(user_id, routing_key, |user, builder| {
            if user.is_admin == admin {
                return Err(IdentityError::InvalidState);
            }
            if admin {
                builder.event(&crate::aggregator::AdminGranted);
            } else {
                builder.event(&AdminRevoked);
            }
            Ok(())
        })
        .await
        .map(|_| ())
        .or_else(|err| match err {
            IdentityError::InvalidState => Ok(()),
            other => Err(other),
        })
    }

    /// Deletes an account: the user is gone for every command and every
    /// session of theirs is void, then their sign-in identities are freed so
    /// the person can register again. The actor cannot delete themself.
    pub async fn delete_user(
        &self,
        actor_id: &str,
        user_id: &str,
        routing_key: Option<String>,
    ) -> Result<(), IdentityError> {
        self.require_admin(actor_id).await?;
        if actor_id == user_id {
            return Err(IdentityError::Forbidden);
        }
        // Including an already deleted user, whose index entries may still be
        // waiting to be freed after a crash.
        let Some(user) = self.load_user_any(user_id).await? else {
            return Err(IdentityError::UserNotFound);
        };
        if !user.deleted {
            self.append_user(user_id, routing_key.clone(), |_, builder| {
                builder.event(&Deleted);
                builder.event(&SessionsRevoked {
                    except_session: None,
                });
                Ok(())
            })
            .await?;
        }
        // `Forbidden` here means someone else registered the identity since
        // (a retry after the first deletion freed it): nothing left to free.
        for account in &user.oauth2_accounts {
            let freed = self
                .unlink_oauth2_index(
                    user_id,
                    &account.provider,
                    &account.provider_user_id,
                    routing_key.clone(),
                )
                .await;
            tolerate_new_owner(freed)?;
        }
        for passkey in &user.passkeys {
            let freed = self
                .remove_passkey_index(user_id, &passkey.credential_id, routing_key.clone())
                .await;
            tolerate_new_owner(freed)?;
        }
        Ok(())
    }
}

fn tolerate_new_owner(result: Result<(), IdentityError>) -> Result<(), IdentityError> {
    match result {
        Ok(()) | Err(IdentityError::Forbidden) => Ok(()),
        Err(err) => Err(err),
    }
}

impl<E: Executor> super::Command<E> {
    /// The acting administrator, or `Forbidden`.
    pub(crate) async fn require_admin(&self, actor_id: &str) -> Result<UserState, IdentityError> {
        match self.load_user(actor_id).await? {
            Some(actor) if actor.is_admin => Ok(actor),
            _ => Err(IdentityError::Forbidden),
        }
    }
}
