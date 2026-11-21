//! The root account as the host's configuration defines it: an address and
//! the hash of a password. Nobody registers it and no page changes it; it
//! is who the host says it is.

use argon2::password_hash::PasswordHash;

use crate::error::IdentityError;
use crate::password::{verify_dummy_password, verify_password};
use crate::value_object::normalize_email;

/// The configured address and password hash of the root account.
#[derive(Clone, PartialEq, Eq)]
pub struct RootAccount {
    email: String,
    password_hash: String,
}

impl std::fmt::Debug for RootAccount {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RootAccount")
            .field("email", &self.email)
            .finish_non_exhaustive()
    }
}

impl RootAccount {
    /// Checks both values as they are read from the configuration, so a
    /// mistyped one stops the start instead of locking root out:
    /// `InvalidEmail` for the address, `MalformedPasswordHash` for a hash
    /// that is not an argon2 PHC string ([`crate::hash_password`] makes
    /// one).
    pub fn new(email: &str, password_hash: &str) -> Result<Self, IdentityError> {
        let email = normalize_email(email)?;
        let is_argon2 = PasswordHash::new(password_hash)
            .is_ok_and(|hash| hash.algorithm.as_str().starts_with("argon2"));
        if !is_argon2 {
            return Err(IdentityError::MalformedPasswordHash);
        }
        Ok(Self {
            email,
            password_hash: password_hash.to_owned(),
        })
    }

    /// The normalized address root signs in with.
    pub fn email(&self) -> &str {
        &self.email
    }

    /// Whether `email`, as someone typed it, is root's address.
    pub fn has_email(&self, email: &str) -> bool {
        normalize_email(email).is_ok_and(|email| email == self.email)
    }

    /// Whether the pair is root's. Any address costs the time of a password
    /// check, so how long the answer takes tells nothing.
    pub(crate) async fn check(&self, email: &str, password: &str) -> Result<bool, IdentityError> {
        if !self.has_email(email) {
            verify_dummy_password(password.to_owned()).await?;
            return Ok(false);
        }
        verify_password(self.password_hash.clone(), password.to_owned()).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::password::hash_password;

    #[tokio::test]
    async fn the_configured_values_are_checked_and_the_pair_verified() {
        let hash = hash_password("correct horse battery").await.unwrap();
        let root = RootAccount::new(" Root@Example.com ", &hash).unwrap();
        assert_eq!(root.email(), "root@example.com");
        assert!(root.has_email("ROOT@example.com"));
        assert!(!root.has_email("other@example.com"));
        assert!(
            !format!("{root:?}").contains("argon2"),
            "the hash stays out"
        );

        assert!(
            root.check("root@example.com", "correct horse battery")
                .await
                .unwrap()
        );
        assert!(!root.check("root@example.com", "wrong").await.unwrap());
        assert!(
            !root
                .check("other@example.com", "correct horse battery")
                .await
                .unwrap()
        );

        let err = RootAccount::new("not an address", &hash).unwrap_err();
        assert!(matches!(err, IdentityError::InvalidEmail), "{err}");
        for bad in ["", "correct horse battery", "$2b$12$abcdefghijklmnopqrstuv"] {
            let err = RootAccount::new("root@example.com", bad).unwrap_err();
            assert!(
                matches!(err, IdentityError::MalformedPasswordHash),
                "{bad:?}: {err}"
            );
        }
    }
}
