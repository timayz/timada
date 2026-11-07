use evento::ProjectionAggregate;
use evento::{Executor, WriteError};

use crate::aggregator::{Claimed, Released};
use crate::error::IdentityError;
use crate::value_object::email_account_id;

use super::{Claiming, Command, email_account_projection, user_projection};

impl<E: Executor> Command<E> {
    /// Points a normalized address at `user_id`, creating or re-claiming the
    /// index entry. Fails when another account signs in with it, or claimed
    /// it within the grace (its add may be committing); an entry nobody holds
    /// (released, its account deleted, or abandoned past the grace) is taken
    /// over. See [`Command::claiming`].
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
                let claiming = self
                    .claiming(
                        existing.released,
                        &existing.user_id,
                        existing.claimed_at_ms,
                        user_id,
                        |user| user.has_email(email),
                    )
                    .await?;
                match claiming {
                    Claiming::Keep => Ok(()),
                    Claiming::Refuse => Err(IdentityError::AlreadyRegistered),
                    Claiming::Take => {
                        existing
                            .write()?
                            .event(&claimed)
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

/// Frees the index entry of an address `user_id` does not sign in with (it
/// was deleted, or the add that claimed the address failed), so the address
/// can be registered again. Same rules as
/// [`super::passkey_index::free_passkey_index`].
pub(crate) async fn free_email_index<E: Executor>(
    executor: &E,
    user_id: &str,
    email: &str,
    let_go_at_ms: Option<u64>,
) -> Result<(), IdentityError> {
    let Some(entry) = email_account_projection()
        .load(email_account_id(email))
        .execute(executor)
        .await?
    else {
        return Ok(());
    };
    if entry.released
        || entry.user_id != user_id
        || let_go_at_ms.is_some_and(|at| entry.claimed_at_ms >= at)
    {
        return Ok(());
    }
    let user = user_projection().load(user_id).execute(executor).await?;
    if user.is_some_and(|user| !user.deleted && user.has_email(email)) {
        return Ok(());
    }
    entry.write()?.event(&Released).commit(executor).await?;
    Ok(())
}
