use evento::ProjectionAggregate;
use evento::{Executor, WriteError};

use crate::aggregator::Created;
use crate::error::IdentityError;
use crate::value_object::passkey_credential_id;

use super::Command;

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
    /// Creates (or re-creates after a removal) the index entry for a passkey.
    /// Fails when the credential is already registered to someone else.
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
            .commit(&self.0)
            .await;
        match result {
            Ok(_) => Ok(()),
            Err(WriteError::InvalidOriginalVersion) => {
                let Some(existing) = self.load_passkey_any(&passkey.credential_id).await? else {
                    return Err(IdentityError::InvalidState);
                };
                if existing.removed {
                    existing.write()?.event(&created).commit(&self.0).await?;
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
}
