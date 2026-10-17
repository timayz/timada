//! The keys a shopper's personal data is sealed under.
//!
//! Every registration draws a fresh random key, and the events record only
//! what it sealed: the address the shopper signs in with and the addresses
//! they ship to. The key never enters the event store: the caller keeps it
//! through [`ShopperKeys`], and destroys it when the shopper deletes their
//! account, which leaves what the events hold unreadable for good.

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use rand::RngCore;

/// The first byte of every sealed value: the layout below.
const VERSION: u8 = 1;
const NONCE_LEN: usize = 12;

/// A 256-bit key, drawn at random for one registration.
#[derive(Clone, PartialEq, Eq)]
pub struct ShopperKey([u8; 32]);

impl std::fmt::Debug for ShopperKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ShopperKey(..)")
    }
}

impl ShopperKey {
    pub fn generate() -> Self {
        let mut bytes = [0u8; 32];
        rand::rng().fill_bytes(&mut bytes);
        Self(bytes)
    }

    /// `None` unless `bytes` is exactly 32 long.
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        Some(Self(bytes.try_into().ok()?))
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

/// A fresh key id: 16 random bytes as URL-safe base64 without padding (22
/// characters of `[A-Za-z0-9_-]`), so it names a file and says nothing of the
/// shopper.
pub fn new_key_id() -> String {
    use base64::Engine;

    let mut bytes = [0u8; 16];
    rand::rng().fill_bytes(&mut bytes);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// Where the keys are kept, for one shop. Every node that serves the shop
/// must be able to read a key once [`put`](Self::put) returned.
#[async_trait::async_trait]
pub trait ShopperKeys: Send + Sync {
    /// Stores a fresh key durably, before any event names it.
    async fn put(&self, key_id: &str, key: &ShopperKey) -> anyhow::Result<()>;

    /// The key, or `None` once destroyed (or never stored). An error means
    /// it cannot be told yet.
    async fn get(&self, key_id: &str) -> anyhow::Result<Option<ShopperKey>>;

    /// Destroys the key for good; one already gone is a no-op.
    async fn delete(&self, key_id: &str) -> anyhow::Result<()>;
}

/// What a sealed value is bound to, so it cannot be moved to another
/// shopper, key or field and still open.
pub(crate) fn aad(shopper_id: &str, key_id: &str, field: &str) -> Vec<u8> {
    let mut aad = Vec::new();
    for part in [shopper_id, key_id, field] {
        aad.extend_from_slice(&(part.len() as u32).to_be_bytes());
        aad.extend_from_slice(part.as_bytes());
    }
    aad
}

/// The version byte, a random nonce and the AES-256-GCM ciphertext.
pub(crate) fn seal(key: &ShopperKey, aad: &[u8], plain: &[u8]) -> anyhow::Result<Vec<u8>> {
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(&key.0));
    let mut nonce = [0u8; NONCE_LEN];
    rand::rng().fill_bytes(&mut nonce);
    let sealed = cipher
        .encrypt(Nonce::from_slice(&nonce), Payload { msg: plain, aad })
        .map_err(|_| anyhow::anyhow!("cannot seal a shopper's data"))?;
    Ok([&[VERSION], nonce.as_slice(), &sealed].concat())
}

/// Fails under another key, another binding, or tampered bytes.
pub(crate) fn open(key: &ShopperKey, aad: &[u8], sealed: &[u8]) -> anyhow::Result<Vec<u8>> {
    anyhow::ensure!(
        sealed.len() > 1 + NONCE_LEN && sealed[0] == VERSION,
        "a shopper's sealed data is malformed"
    );
    let (nonce, body) = sealed[1..].split_at(NONCE_LEN);
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(&key.0));
    cipher
        .decrypt(Nonce::from_slice(nonce), Payload { msg: body, aad })
        .map_err(|_| anyhow::anyhow!("a shopper's sealed data does not open under its key"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_value_opens_under_its_key_and_binding_only() {
        let key = ShopperKey::generate();
        let binding = aad("shopper", "key", "email");
        let sealed = seal(&key, &binding, b"ada@example.com").unwrap();
        assert!(!sealed.windows(15).any(|w| w == b"ada@example.com"));
        assert_eq!(open(&key, &binding, &sealed).unwrap(), b"ada@example.com");
        assert_ne!(seal(&key, &binding, b"ada@example.com").unwrap(), sealed);

        assert!(open(&ShopperKey::generate(), &binding, &sealed).is_err());
        assert!(open(&key, &aad("other", "key", "email"), &sealed).is_err());
        assert!(open(&key, &aad("shopper", "key", "address:1"), &sealed).is_err());
        let mut tampered = sealed.clone();
        *tampered.last_mut().unwrap() ^= 1;
        assert!(open(&key, &binding, &tampered).is_err());
        assert!(open(&key, &binding, &sealed[..5]).is_err());
    }

    #[test]
    fn the_binding_parts_cannot_slide() {
        assert_ne!(aad("ab", "c", "d"), aad("a", "bc", "d"));
    }

    #[test]
    fn key_ids_name_a_file() {
        let id = new_key_id();
        assert_eq!(id.len(), 22);
        assert!(
            id.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        );
        assert_ne!(new_key_id(), id);
        assert_eq!(ShopperKey::from_bytes(&[0; 31]), None);
        assert!(format!("{:?}", ShopperKey::generate()).ends_with("(..)"));
    }
}
