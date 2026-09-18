use argon2::{
    Argon2, PasswordHash, PasswordHasher, PasswordVerifier,
    password_hash::{SaltString, rand_core::OsRng},
};

use std::sync::LazyLock;

/// A hash of a throwaway password, verified against when the account does not
/// exist so timing does not reveal which emails are taken.
pub static DUMMY_HASH: LazyLock<String> =
    LazyLock::new(|| hash("not-a-real-password").unwrap_or_default());

pub fn hash(password: &str) -> Option<String> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .ok()
        .map(|h| h.to_string())
}

pub fn verify(password: &str, hash: &str) -> bool {
    PasswordHash::new(hash)
        .map(|parsed| {
            Argon2::default()
                .verify_password(password.as_bytes(), &parsed)
                .is_ok()
        })
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        let hashed = hash("hunter2").unwrap_or_default();
        assert!(verify("hunter2", &hashed));
        assert!(!verify("hunter3", &hashed));
        assert!(!verify("anything", &DUMMY_HASH));
    }
}
