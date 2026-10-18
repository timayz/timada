//! Encrypts the tenants' secrets (payment keys, mail server passwords)
//! before they are recorded, and the personal-data keys before they are
//! written to disk. The events replicate to every node and live
//! forever, so what they hold is AES-256-GCM ciphertext under a key derived
//! from the operator's secret: a random 12-byte nonce followed by the sealed
//! bytes.

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use rand::RngCore;
use sha2::{Digest, Sha256};

use crate::config::Config;

const NONCE_LEN: usize = 12;

/// What seals the tenants' payment secrets: `payments.secret_key`, or else
/// `secrets.key`. `None` when payments are off.
pub fn payments_sealer(config: &Config) -> Option<Sealer> {
    let payments = config.payments.as_ref()?;
    let key = payments
        .secret_key
        .as_deref()
        .or(config.secrets.as_ref().map(|secrets| secrets.key.as_str()))?;
    Some(Sealer::new("payments", key))
}

/// What seals the shops' mail server passwords: `None` without
/// `secrets.key`.
pub fn mail_sealer(config: &Config) -> Option<Sealer> {
    let secrets = config.secrets.as_ref()?;
    Some(Sealer::new("mail", &secrets.key))
}

/// What seals the personal-data keys on disk ([`crate::key_vault`]): `None`
/// without `secrets.key`. The purpose stays `shoppers`, its first use: it
/// labels the derived key, and the files already on disk were sealed under it.
pub fn keys_sealer(config: &Config) -> Option<Sealer> {
    let secrets = config.secrets.as_ref()?;
    Some(Sealer::new("shoppers", &secrets.key))
}

#[derive(Clone)]
pub struct Sealer {
    cipher: Aes256Gcm,
    purpose: &'static str,
}

impl std::fmt::Debug for Sealer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Sealer(..)")
    }
}

impl Sealer {
    /// The key is the SHA-256 of the operator's secret under a label naming
    /// `purpose` (`payments`, `mail`), so each purpose gets a key of its own
    /// from the same secret.
    pub fn new(purpose: &'static str, secret: &str) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(b"timada/");
        hasher.update(purpose.as_bytes());
        hasher.update(b"/seal\0");
        hasher.update(secret.as_bytes());
        let key = hasher.finalize();
        Self {
            cipher: Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(&key)),
            purpose,
        }
    }

    pub fn seal(&self, plain: &str) -> anyhow::Result<Vec<u8>> {
        self.seal_bytes(plain.as_bytes())
    }

    pub fn seal_bytes(&self, plain: &[u8]) -> anyhow::Result<Vec<u8>> {
        let mut nonce = [0u8; NONCE_LEN];
        rand::rng().fill_bytes(&mut nonce);
        let sealed = self
            .cipher
            .encrypt(Nonce::from_slice(&nonce), plain)
            .map_err(|_| anyhow::anyhow!("cannot seal a {} secret", self.purpose))?;
        Ok([nonce.as_slice(), &sealed].concat())
    }

    /// Fails when the bytes were sealed under another key or tampered with.
    pub fn open(&self, sealed: &[u8]) -> anyhow::Result<String> {
        Ok(String::from_utf8(self.open_bytes(sealed)?)?)
    }

    /// Fails when the bytes were sealed under another key or tampered with.
    pub fn open_bytes(&self, sealed: &[u8]) -> anyhow::Result<Vec<u8>> {
        anyhow::ensure!(
            sealed.len() > NONCE_LEN,
            "a sealed {} secret is too short",
            self.purpose
        );
        let (nonce, body) = sealed.split_at(NONCE_LEN);
        self.cipher
            .decrypt(Nonce::from_slice(nonce), body)
            .map_err(|_| {
                anyhow::anyhow!(
                    "a {} secret cannot be opened (was the sealing key changed?)",
                    self.purpose
                )
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_secret_opens_under_its_key_only() {
        let sealer = Sealer::new("payments", "a-test-secret-of-thirty-two-chars!!");
        let sealed = sealer.seal("sk_test_123").unwrap();
        assert!(!sealed.windows(11).any(|w| w == b"sk_test_123"));
        assert_eq!(sealer.open(&sealed).unwrap(), "sk_test_123");
        assert_ne!(
            sealer.seal("sk_test_123").unwrap(),
            sealed,
            "a fresh nonce each time"
        );

        let other = Sealer::new("payments", "another-secret-of-thirty-two-chars!");
        assert!(other.open(&sealed).is_err());
        let mail = Sealer::new("mail", "a-test-secret-of-thirty-two-chars!!");
        assert!(mail.open(&sealed).is_err(), "another purpose, another key");
        let shoppers = Sealer::new("shoppers", "a-test-secret-of-thirty-two-chars!!");
        assert!(
            shoppers.open(&sealed).is_err(),
            "another purpose, another key"
        );
        let key = [7u8; 32];
        assert_eq!(
            shoppers
                .open_bytes(&shoppers.seal_bytes(&key).unwrap())
                .unwrap(),
            key
        );
        let mut tampered = sealed.clone();
        *tampered.last_mut().unwrap() ^= 1;
        assert!(sealer.open(&tampered).is_err());
    }

    /// The payment secrets recorded before the sealer took a purpose still
    /// open: the label is byte for byte the one it used.
    #[test]
    fn payments_keep_their_key() {
        let secret = "a-test-secret-of-thirty-two-chars!!";
        let mut hasher = Sha256::new();
        hasher.update(b"timada/payments/seal\0");
        hasher.update(secret.as_bytes());
        let old = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(&hasher.finalize()));
        let nonce = [7u8; NONCE_LEN];
        let body = old
            .encrypt(Nonce::from_slice(&nonce), b"sk_test_old".as_slice())
            .unwrap();
        let sealed = [nonce.as_slice(), &body].concat();
        assert_eq!(
            Sealer::new("payments", secret).open(&sealed).unwrap(),
            "sk_test_old"
        );
    }
}
