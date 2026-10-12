use evento::Executor;

use crate::error::IdentityError;
use crate::password::{verify_dummy_password, verify_password};
use crate::value_object::normalize_email;

/// The user behind a correct e-mail + password pair.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PasswordOwner {
    pub user_id: String,
    pub label: String,
}

impl<E: Executor> super::Command<E> {
    /// The first half of a password sign-in: who the credentials belong to.
    /// Nothing is recorded; the sign-in happens once the mailed code is back.
    /// An unknown address costs the same time as a wrong password.
    pub async fn check_password(
        &self,
        email: &str,
        password: &str,
    ) -> Result<PasswordOwner, IdentityError> {
        let Ok(email) = normalize_email(email) else {
            verify_dummy_password(password.to_owned()).await?;
            return Err(IdentityError::InvalidCredentials);
        };
        let owner = match self.load_email_account(&email).await? {
            Some(owner) => self.load_user(&owner.user_id).await?,
            None => None,
        };
        let Some(hash) = owner.as_ref().and_then(|u| u.password.as_ref()) else {
            verify_dummy_password(password.to_owned()).await?;
            return Err(IdentityError::InvalidCredentials);
        };
        if !verify_password(hash.password_hash.clone(), password.to_owned()).await? {
            return Err(IdentityError::InvalidCredentials);
        }
        let user = owner.expect("checked above");
        Ok(PasswordOwner {
            user_id: user.id,
            label: user.label,
        })
    }
}
