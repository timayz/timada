use evento::Executor;

use crate::aggregator::PasskeyRenamed;
use crate::error::IdentityError;

#[evento::command]
impl<E: Executor> super::Command<E> {
    pub async fn rename_passkey(
        &self,
        user_id: &str,
        credential_id: &str,
        name: &str,
        routing_key: Option<String>,
    ) -> Result<(), IdentityError> {
        let name = name.trim();
        if name.is_empty() || name.chars().count() > 64 {
            return Err(IdentityError::InvalidState);
        }
        self.append_user(user_id, routing_key, |user, builder| {
            let Some(passkey) = user.passkey(credential_id) else {
                return Err(IdentityError::Forbidden);
            };
            if passkey.name == name {
                return Err(IdentityError::InvalidState);
            }
            builder.event(&PasskeyRenamed {
                credential_id: credential_id.to_owned(),
                name: name.to_owned(),
            });
            Ok(())
        })
        .await
        .map(|_| ())
        .or_else(|err| match err {
            IdentityError::InvalidState => Ok(()),
            other => Err(other),
        })
    }
}
