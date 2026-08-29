//! Argon2id hashing behind two functions. Hashes are PHC strings, so the
//! parameters travel with each hash and can be strengthened for new hashes
//! without invalidating old ones.

use argon2::Argon2;
use argon2::password_hash::rand_core::OsRng;
use argon2::password_hash::{PasswordHash, PasswordHasher as _, PasswordVerifier as _, SaltString};

/// Hash `plain` with Argon2id and a fresh random salt.
pub fn hash_password(plain: &str) -> anyhow::Result<String> {
    let salt = SaltString::generate(&mut OsRng);
    let hash = Argon2::default()
        .hash_password(plain.as_bytes(), &salt)
        .map_err(|source| anyhow::anyhow!("failed to hash password: {source}"))?;
    Ok(hash.to_string())
}

/// Check `plain` against a stored PHC hash. A wrong password is `Ok(false)`;
/// only a malformed hash or an internal failure is an error.
pub fn verify_password(plain: &str, phc: &str) -> anyhow::Result<bool> {
    let parsed = PasswordHash::new(phc)
        .map_err(|source| anyhow::anyhow!("stored password hash is malformed: {source}"))?;
    match Argon2::default().verify_password(plain.as_bytes(), &parsed) {
        Ok(()) => Ok(true),
        Err(argon2::password_hash::Error::Password) => Ok(false),
        Err(source) => Err(anyhow::anyhow!("failed to verify password: {source}")),
    }
}
