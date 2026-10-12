//! Password hashing (argon2id, stored as PHC strings) and the one-time codes
//! mailed to prove an address. Hashing is CPU-bound by design, so it runs on
//! the blocking pool and never stalls the executor's threads.

use std::sync::LazyLock;

use argon2::Argon2;
use argon2::password_hash::rand_core::{OsRng, RngCore};
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};

use crate::error::IdentityError;
use crate::value_object::{PASSWORD_MAX_LEN, PASSWORD_MIN_LEN};

/// Checks the length policy and hashes the password.
pub async fn hash_password(password: &str) -> Result<String, IdentityError> {
    let length = password.chars().count();
    if !(PASSWORD_MIN_LEN..=PASSWORD_MAX_LEN).contains(&length) {
        return Err(IdentityError::WeakPassword);
    }
    let password = password.to_owned();
    tokio::task::spawn_blocking(move || hash_blocking(&password))
        .await
        .map_err(|err| IdentityError::Other(err.into()))?
}

/// Whether `password` is the one behind `hash`. A malformed hash is an
/// infrastructure error, not a wrong password.
pub async fn verify_password(hash: String, password: String) -> Result<bool, IdentityError> {
    tokio::task::spawn_blocking(move || verify_blocking(&hash, &password))
        .await
        .map_err(|err| IdentityError::Other(err.into()))?
}

/// Burns the time a real verification would take, so an unknown address and
/// a wrong password answer alike.
pub(crate) async fn verify_dummy_password(password: String) -> Result<(), IdentityError> {
    verify_password(DUMMY_HASH.clone(), password).await?;
    Ok(())
}

static DUMMY_HASH: LazyLock<String> = LazyLock::new(|| {
    hash_blocking("timing-placeholder-never-a-real-password")
        .expect("argon2 with default parameters cannot fail")
});

fn hash_blocking(password: &str) -> Result<String, IdentityError> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|hash| hash.to_string())
        .map_err(|err| IdentityError::Other(anyhow::anyhow!("cannot hash the password: {err}")))
}

fn verify_blocking(hash: &str, password: &str) -> Result<bool, IdentityError> {
    let parsed = PasswordHash::new(hash)
        .map_err(|err| anyhow::anyhow!("the stored password hash is malformed: {err}"))?;
    Ok(Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok())
}

/// A uniformly random six-digit code, zero-padded.
pub(crate) fn generate_code() -> String {
    // Rejection sampling: the largest multiple of 10^6 below 2^32.
    const LIMIT: u32 = 4_294_000_000;
    loop {
        let n = OsRng.next_u32();
        if n < LIMIT {
            return format!("{:06}", n % 1_000_000);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn hashes_round_trip_and_follow_the_policy() {
        let hash = hash_password("correct horse battery").await.unwrap();
        assert!(hash.starts_with("$argon2id$"), "{hash}");
        assert!(
            verify_password(hash.clone(), "correct horse battery".to_owned())
                .await
                .unwrap()
        );
        assert!(!verify_password(hash, "wrong".to_owned()).await.unwrap());

        let err = hash_password("short").await.unwrap_err();
        assert!(matches!(err, IdentityError::WeakPassword), "{err}");
        let err = hash_password(&"x".repeat(129)).await.unwrap_err();
        assert!(matches!(err, IdentityError::WeakPassword), "{err}");
        assert!(
            hash_password(&"é".repeat(8)).await.is_ok(),
            "counts characters"
        );

        let err = verify_password("not-a-hash".to_owned(), "x".to_owned())
            .await
            .unwrap_err();
        assert!(!err.is_user_facing());
    }

    #[test]
    fn codes_are_six_digits() {
        for _ in 0..100 {
            let code = generate_code();
            assert_eq!(code.len(), 6);
            assert!(code.bytes().all(|b| b.is_ascii_digit()), "{code}");
        }
    }
}
