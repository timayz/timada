//! Administration: roles and account deletion. Every command checks that the
//! actor administers the instance; the first registered user does by default.

use evento::Executor;

use crate::aggregator::{AdminRevoked, Deleted, SessionsRevoked};
use crate::error::IdentityError;

use super::UserState;
use super::email_index::free_email_index;
use super::oauth2_index::free_oauth2_index;
use super::passkey_index::free_passkey_index;

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
    /// the person can register again (by the index-release subscription
    /// should that fail; a deleted user holds nothing meanwhile). The actor
    /// cannot delete themself.
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
            self.append_user(user_id, routing_key, |_, builder| {
                builder.event(&Deleted);
                builder.event(&SessionsRevoked {
                    except_session: None,
                });
                Ok(())
            })
            .await?;
        }
        if let Err(err) = free_indexes(&self.executor, &user, None).await {
            tracing::warn!(%err, user_id, "the deleted user's index entries are freed later");
        }
        Ok(())
    }
}

/// Frees every index entry of a deleted user's sign-in identities, each as
/// [`free_passkey_index`] does; the first failure is returned once all were
/// tried.
pub(crate) async fn free_indexes<E: Executor>(
    executor: &E,
    user: &UserState,
    let_go_at_ms: Option<u64>,
) -> Result<(), IdentityError> {
    let mut outcome = Ok(());
    for account in &user.oauth2_accounts {
        let freed = free_oauth2_index(
            executor,
            &user.id,
            &account.provider,
            &account.provider_user_id,
            let_go_at_ms,
        )
        .await;
        outcome = outcome.and(freed);
    }
    for passkey in &user.passkeys {
        let freed =
            free_passkey_index(executor, &user.id, &passkey.credential_id, let_go_at_ms).await;
        outcome = outcome.and(freed);
    }
    if let Some(password) = &user.password {
        let freed = free_email_index(executor, &user.id, &password.email, let_go_at_ms).await;
        outcome = outcome.and(freed);
    }
    outcome
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
