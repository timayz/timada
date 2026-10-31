//! The one-time codes mailed to prove an address, and what a stream may keep
//! of them.
//!
//! A six-digit code is guessable under any plain hash: whoever reads the
//! stream (a storefront's key reads a tenant's whole store) tries the million
//! codes offline and signs in as the shopper, past the attempt cap the stream
//! counts. So a stream keeps a MAC of the code under a key that never enters
//! the store: the host derives it from the operator's secret and hands it to
//! the domain as a [`CodeVerifier`]. Without the key, the tag says nothing
//! about the code; the stream still counts the guesses.

use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};

type HmacSha256 = Hmac<Sha256>;

/// Turns a mailed code into the tag its stream keeps, and checks a guess
/// against it. The tag is bound to the tenant and the challenge, so a tag
/// copied from one stream proves nothing on another.
pub trait CodeVerifier: Send + Sync {
    /// What the stream keeps for `code`.
    fn tag(&self, tenant: &str, challenge_id: &str, code: &str) -> Vec<u8>;

    /// Whether `code` is the one `tag` was made from. Constant time.
    fn verify(&self, tenant: &str, challenge_id: &str, code: &str, tag: &[u8]) -> bool;
}

/// HMAC-SHA256 under a key the host holds.
#[derive(Clone)]
pub struct HmacCodeVerifier {
    key: [u8; 32],
}

impl std::fmt::Debug for HmacCodeVerifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("HmacCodeVerifier(..)")
    }
}

impl HmacCodeVerifier {
    pub fn new(key: [u8; 32]) -> Self {
        Self { key }
    }

    /// A key derived from the operator's `secret` under a label naming
    /// `purpose` (`shopper-code`, `email-code`), so each purpose gets a key
    /// of its own from the same secret, apart from the sealing keys.
    pub fn derive(purpose: &str, secret: &str) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(b"timada/");
        hasher.update(purpose.as_bytes());
        hasher.update(b"/code-mac\0");
        hasher.update(secret.as_bytes());
        Self {
            key: hasher.finalize().into(),
        }
    }

    fn mac(&self, tenant: &str, challenge_id: &str, code: &str) -> HmacSha256 {
        let mut mac = HmacSha256::new_from_slice(&self.key).expect("any key length works");
        for part in [tenant, challenge_id, code.trim()] {
            mac.update(&(part.len() as u32).to_be_bytes());
            mac.update(part.as_bytes());
        }
        mac
    }
}

impl CodeVerifier for HmacCodeVerifier {
    fn tag(&self, tenant: &str, challenge_id: &str, code: &str) -> Vec<u8> {
        self.mac(tenant, challenge_id, code)
            .finalize()
            .into_bytes()
            .to_vec()
    }

    fn verify(&self, tenant: &str, challenge_id: &str, code: &str, tag: &[u8]) -> bool {
        self.mac(tenant, challenge_id, code)
            .verify_slice(tag)
            .is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tag_opens_only_under_its_key_tenant_and_challenge() {
        let verifier = HmacCodeVerifier::new([7; 32]);
        let tag = verifier.tag("acme", "challenge-1", "123456");
        assert_eq!(tag.len(), 32);
        assert!(verifier.verify("acme", "challenge-1", "123456", &tag));
        assert!(verifier.verify("acme", "challenge-1", " 123456 ", &tag));
        assert!(!verifier.verify("acme", "challenge-1", "123457", &tag));
        assert!(!verifier.verify("other", "challenge-1", "123456", &tag));
        assert!(!verifier.verify("acme", "challenge-2", "123456", &tag));
        assert!(!HmacCodeVerifier::new([8; 32]).verify("acme", "challenge-1", "123456", &tag));
        assert!(!verifier.verify("acme", "challenge-1", "123456", &tag[..31]));
    }

    #[test]
    fn a_purpose_derives_its_own_key() {
        let shoppers = HmacCodeVerifier::derive("shopper-code", "a-secret");
        let email = HmacCodeVerifier::derive("email-code", "a-secret");
        let tag = shoppers.tag("", "c", "000000");
        assert!(shoppers.verify("", "c", "000000", &tag));
        assert!(!email.verify("", "c", "000000", &tag));
        assert!(
            !HmacCodeVerifier::derive("shopper-code", "another").verify("", "c", "000000", &tag)
        );
    }
}
