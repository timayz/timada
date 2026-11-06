use evento::ProjectionAggregate;
use evento::{Executor, WriteError};

use crate::aggregator::{Claimed, Released};
use crate::error::IdentityError;
use crate::value_object::email_account_id;

use super::{Command, email_account_projection, user_projection};

impl<E: Executor> Command<E> {
    /// Points a normalized address at `user_id`, creating or re-claiming the
    /// index entry. Fails when another user holds it; an entry nobody holds
    /// (released, or left behind by an account that never recorded the
    /// address or was deleted) is taken over.
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
                match self.email_holder(&existing).await? {
                    Some(holder) if holder.id == user_id => Ok(()),
                    Some(_) => Err(IdentityError::AlreadyRegistered),
                    None => {
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
