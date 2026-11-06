use evento::ProjectionAggregate;
use evento::{Executor, WriteError};

use crate::aggregator::{Created, Removed};
use crate::error::IdentityError;
use crate::value_object::passkey_credential_id;

use super::{Command, passkey_credential_projection, user_projection};

/// A credential the web layer has verified and serialized.
#[derive(Debug, Clone)]
pub struct VerifiedPasskey {
    /// base64url credential id, as the authenticator reports it.
    pub credential_id: String,
    pub passkey_json: String,
    pub aaguid: String,
    pub rp_id: String,
    pub name: String,
}

impl<E: Executor> Command<E> {
    /// Creates (or re-creates) the index entry for a passkey. Fails when
    /// another user holds the credential; an entry nobody holds (removed, or
    /// left behind by an account that let go of it) is taken over.
    pub(crate) async fn create_passkey_index(
        &self,
        passkey: &VerifiedPasskey,
        user_id: &str,
        routing_key: Option<String>,
    ) -> Result<(), IdentityError> {
        let created = Created {
            user_id: user_id.to_owned(),
            credential_id: passkey.credential_id.clone(),
            passkey_json: passkey.passkey_json.clone(),
            aaguid: passkey.aaguid.clone(),
            rp_id: passkey.rp_id.clone(),
            name: passkey.name.clone(),
        };
        let result = evento::append(passkey_credential_id(&passkey.credential_id))
            .original_version(0)
            .routing_key_opt(routing_key)
            .event(&created)
            .commit(&self.executor)
            .await;
        match result {
            Ok(_) => Ok(()),
            Err(WriteError::InvalidOriginalVersion) => {
                let Some(existing) = self.load_passkey_any(&passkey.credential_id).await? else {
                    return Err(IdentityError::InvalidState);
                };
                match self.passkey_holder(&existing).await? {
                    Some(holder) if holder.id == user_id => Ok(()),
                    Some(_) => Err(IdentityError::AlreadyRegistered),
                    None => {
                        existing
                            .write()?
                            .event(&created)
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

/// Frees the index entry of a passkey `user_id` let go of (removed it, or was
/// deleted), so the credential can be registered again. An entry already
/// free, another user's, or listed on the account again is left alone, so
/// telling it twice changes nothing; so is one claimed at or after
/// `let_go_at_ms`, which a later add made. One attempt: a race with another
/// writer of the entry is a [`WriteError::InvalidOriginalVersion`].
pub(crate) async fn free_passkey_index<E: Executor>(
    executor: &E,
    user_id: &str,
    credential_id: &str,
    let_go_at_ms: Option<u64>,
) -> Result<(), IdentityError> {
    let Some(entry) = passkey_credential_projection()
        .load(passkey_credential_id(credential_id))
        .execute(executor)
        .await?
    else {
        return Ok(());
    };
    if entry.removed
        || entry.user_id != user_id
        || let_go_at_ms.is_some_and(|at| entry.claimed_at_ms >= at)
    {
        return Ok(());
    }
    let user = user_projection().load(user_id).execute(executor).await?;
    if user.is_some_and(|user| !user.deleted && user.passkey(credential_id).is_some()) {
        return Ok(());
    }
    entry.write()?.event(&Removed).commit(executor).await?;
    Ok(())
}
