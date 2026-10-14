//! Encrypts the tenants' payment secrets before they are recorded. The
//! events replicate to every node and live forever, so what they hold is
//! AES-256-GCM ciphertext under a key derived from `payments.secret_key`:
//! a random 12-byte nonce followed by the sealed bytes.

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use rand::RngCore;
use sha2::{Digest, Sha256};

const NONCE_LEN: usize = 12;

#[derive(Clone)]
pub struct Sealer {
    cipher: Aes256Gcm,
}

impl std::fmt::Debug for Sealer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Sealer(..)")
    }
}

impl Sealer {
    /// The key is the SHA-256 of the operator's secret, under a label of its
    /// own so the same secret used elsewhere gives another key.
    pub fn new(secret: &str) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(b"timada/payments/seal\0");
        hasher.update(secret.as_bytes());
        let key = hasher.finalize();
        Self {
            cipher: Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(&key)),
        }
    }

    pub fn seal(&self, plain: &str) -> anyhow::Result<Vec<u8>> {
        let mut nonce = [0u8; NONCE_LEN];
        rand::rng().fill_bytes(&mut nonce);
        let sealed = self
            .cipher
            .encrypt(Nonce::from_slice(&nonce), plain.as_bytes())
            .map_err(|_| anyhow::anyhow!("cannot seal a payment secret"))?;
        Ok([nonce.as_slice(), &sealed].concat())
    }

    /// Fails when the bytes were sealed under another key or tampered with.
    pub fn open(&self, sealed: &[u8]) -> anyhow::Result<String> {
        anyhow::ensure!(
            sealed.len() > NONCE_LEN,
            "a sealed payment secret is too short"
        );
        let (nonce, body) = sealed.split_at(NONCE_LEN);
        let plain = self
            .cipher
            .decrypt(Nonce::from_slice(nonce), body)
            .map_err(|_| {
                anyhow::anyhow!(
                    "a payment secret cannot be opened (was payments.secret_key changed?)"
                )
            })?;
        Ok(String::from_utf8(plain)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_secret_opens_under_its_key_only() {
        let sealer = Sealer::new("a-test-secret-of-thirty-two-chars!!");
        let sealed = sealer.seal("sk_test_123").unwrap();
        assert!(!sealed.windows(11).any(|w| w == b"sk_test_123"));
        assert_eq!(sealer.open(&sealed).unwrap(), "sk_test_123");
        assert_ne!(
            sealer.seal("sk_test_123").unwrap(),
            sealed,
            "a fresh nonce each time"
        );

        let other = Sealer::new("another-secret-of-thirty-two-chars!");
        assert!(other.open(&sealed).is_err());
        let mut tampered = sealed.clone();
        *tampered.last_mut().unwrap() ^= 1;
        assert!(sealer.open(&tampered).is_err());
    }
}
