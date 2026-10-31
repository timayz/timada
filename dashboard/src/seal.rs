//! Encrypts the tenants' secrets (payment keys, mail server passwords)
//! before they are recorded, and the personal-data keys before they are
//! written to disk. The events replicate to every node and live
//! forever, so what they hold is AES-256-GCM ciphertext under a key derived
//! from the operator's secret: a random 12-byte nonce followed by the sealed
//! bytes.

use std::sync::Arc;

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use rand::RngCore;
use sha2::{Digest, Sha256};
use timada_core::{CodeVerifier, HmacCodeVerifier};

use crate::config::Config;

/// What checks the codes mailed to the platform's users against what their
/// streams keep: a MAC under a key derived from `auth.secret_key`, which
/// every node shares and the store never holds.
#[derive(Clone)]
pub struct EmailCodes(pub Arc<dyn CodeVerifier>);

impl std::fmt::Debug for EmailCodes {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("EmailCodes(..)")
    }
}

pub fn email_codes(config: &Config) -> EmailCodes {
    EmailCodes(Arc::new(HmacCodeVerifier::derive(
        "email-code",
        &config.auth.secret_key,
    )))
}

/// The same for the shops' shoppers, under `secrets.key`: `None` without it
/// (no shopper accounts then). A storefront's key reads the tenant's whole
/// store, challenge streams included, so what they keep must say nothing
/// without this.
#[derive(Clone)]
pub struct ShopperCodes(pub Option<Arc<dyn CodeVerifier>>);

impl std::fmt::Debug for ShopperCodes {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ShopperCodes(..)")
    }
}

pub fn shopper_codes(config: &Config) -> ShopperCodes {
    ShopperCodes(config.secrets.as_ref().map(|secrets| {
        Arc::new(HmacCodeVerifier::derive("shopper-code", &secrets.key)) as Arc<dyn CodeVerifier>
    }))
}

const NONCE_LEN: usize = 12;

/// What seals the tenants' payment secrets: `None` when payments are off or
/// without `secrets.key`.
pub fn payments_sealer(config: &Config) -> Option<Sealer> {
    config.payments.as_ref()?;
    let secrets = config.secrets.as_ref()?;
    Some(Sealer::new("payments", &secrets.key))
}

/// What seals the shops' mail server passwords: `None` without
/// `secrets.key`.
pub fn mail_sealer(config: &Config) -> Option<Sealer> {
    let secrets = config.secrets.as_ref()?;
    Some(Sealer::new("mail", &secrets.key))
}

/// What seals the tokens of the companies' AliExpress accounts: `None`
/// without `secrets.key`.
pub fn aliexpress_sealer(config: &Config) -> Option<Sealer> {
    let secrets = config.secrets.as_ref()?;
    Some(Sealer::new("aliexpress", &secrets.key))
}

/// What seals the personal-data keys on disk ([`crate::key_vault`]): `None`
/// without `secrets.key`. The purpose stays `shoppers`, its first use: it
/// labels the derived key, and the files already on disk were sealed under it.
pub fn keys_sealer(config: &Config) -> Option<Sealer> {
    let secrets = config.secrets.as_ref()?;
    Some(Sealer::new("shoppers", &secrets.key))
}

/// The key the links of the shops' automated mails are signed under, so the
/// opt-out page can tell one it mailed: `None` without `secrets.key`. Every
/// node derives the same from the operator's secret; nothing is stored.
pub fn opt_out_key(config: &Config) -> Option<OptOutKey> {
    let secrets = config.secrets.as_ref()?;
    let mut hasher = Sha256::new();
    hasher.update(b"timada/automation-opt-out/mac\0");
    hasher.update(secrets.key.as_bytes());
    Some(OptOutKey(hasher.finalize().into()))
}

/// What the shops' webhook steps need: a sealer for their addresses and the
/// key their signing secrets derive from; `None` without `secrets.key`.
pub fn webhook_keys(config: &Config) -> Option<WebhookKeys> {
    let secrets = config.secrets.as_ref()?;
    let mut hasher = Sha256::new();
    hasher.update(b"timada/automation-webhook/sign\0");
    hasher.update(secrets.key.as_bytes());
    Some(WebhookKeys {
        sealer: Sealer::new("automation-webhook", &secrets.key),
        signing: hasher.finalize().into(),
    })
}

/// See [`webhook_keys`].
#[derive(Clone)]
pub struct WebhookKeys {
    pub sealer: Sealer,
    pub signing: [u8; 32],
}

impl std::fmt::Debug for WebhookKeys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("WebhookKeys(..)")
    }
}

/// See [`opt_out_key`].
#[derive(Clone, Copy)]
pub struct OptOutKey(pub [u8; 32]);

impl std::fmt::Debug for OptOutKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("OptOutKey(..)")
    }
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

    /// Seals `plain` for `owner`: the tenant, company or key the secret
    /// belongs to, bound into the ciphertext as authenticated data, so bytes
    /// copied from one owner's events into another's do not open there.
    pub fn seal_for(&self, owner: &str, plain: &str) -> anyhow::Result<Vec<u8>> {
        self.seal_bytes_for(owner, plain.as_bytes())
    }

    /// See [`seal_for`](Self::seal_for).
    pub fn seal_bytes_for(&self, owner: &str, plain: &[u8]) -> anyhow::Result<Vec<u8>> {
        let mut nonce = [0u8; NONCE_LEN];
        rand::rng().fill_bytes(&mut nonce);
        let sealed = self
            .cipher
            .encrypt(
                Nonce::from_slice(&nonce),
                Payload {
                    msg: plain,
                    aad: owner.as_bytes(),
                },
            )
            .map_err(|_| anyhow::anyhow!("cannot seal a {} secret", self.purpose))?;
        Ok([nonce.as_slice(), &sealed].concat())
    }

    /// Fails when the bytes were sealed under another key, for another
    /// owner, or tampered with.
    pub fn open_for(&self, owner: &str, sealed: &[u8]) -> anyhow::Result<String> {
        Ok(String::from_utf8(self.open_bytes_for(owner, sealed)?)?)
    }

    /// See [`open_for`](Self::open_for).
    pub fn open_bytes_for(&self, owner: &str, sealed: &[u8]) -> anyhow::Result<Vec<u8>> {
        anyhow::ensure!(
            sealed.len() > NONCE_LEN,
            "a sealed {} secret is too short",
            self.purpose
        );
        let (nonce, body) = sealed.split_at(NONCE_LEN);
        self.cipher
            .decrypt(
                Nonce::from_slice(nonce),
                Payload {
                    msg: body,
                    aad: owner.as_bytes(),
                },
            )
            .map_err(|_| {
                anyhow::anyhow!(
                    "a {} secret cannot be opened (sealed for another owner, or was the sealing key changed?)",
                    self.purpose
                )
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_secret_opens_under_its_key_for_its_owner_only() {
        let sealer = Sealer::new("payments", "a-test-secret-of-thirty-two-chars!!");
        let sealed = sealer.seal_for("tenant-1", "sk_test_123").unwrap();
        assert!(!sealed.windows(11).any(|w| w == b"sk_test_123"));
        assert!(
            !sealed.windows(8).any(|w| w == b"tenant-1"),
            "the owner is not written"
        );
        assert_eq!(sealer.open_for("tenant-1", &sealed).unwrap(), "sk_test_123");
        assert_ne!(
            sealer.seal_for("tenant-1", "sk_test_123").unwrap(),
            sealed,
            "a fresh nonce each time"
        );
        // Copied into another tenant's events, the bytes open nothing.
        assert!(sealer.open_for("tenant-2", &sealed).is_err());
        assert!(sealer.open_for("", &sealed).is_err());

        let other = Sealer::new("payments", "another-secret-of-thirty-two-chars!");
        assert!(other.open_for("tenant-1", &sealed).is_err());
        let mail = Sealer::new("mail", "a-test-secret-of-thirty-two-chars!!");
        assert!(
            mail.open_for("tenant-1", &sealed).is_err(),
            "another purpose, another key"
        );
        let shoppers = Sealer::new("shoppers", "a-test-secret-of-thirty-two-chars!!");
        let key = [7u8; 32];
        assert_eq!(
            shoppers
                .open_bytes_for("t1/k1", &shoppers.seal_bytes_for("t1/k1", &key).unwrap())
                .unwrap(),
            key
        );
        let mut tampered = sealed.clone();
        *tampered.last_mut().unwrap() ^= 1;
        assert!(sealer.open_for("tenant-1", &tampered).is_err());
    }
}
