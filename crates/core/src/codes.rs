//! The one-time codes mailed to prove an address, the tokens mailed as
//! links, and what a stream may keep of them.
//!
//! A six-digit code is guessable under any plain hash: whoever reads the
//! stream (a storefront's key reads a tenant's whole store) tries the million
//! codes offline and signs in as the shopper, past the attempt cap the stream
//! counts. So a stream keeps a random *nonce*, and the code is derived from
//! it by a MAC under a key that never enters the store: the host derives the
//! key from the operator's secret and hands it to the domain as a
//! [`CodeVerifier`]. Whoever mails the code (a subscription on another node,
//! say) derives it from the nonce the event carries; verification derives it
//! again and compares. Without the key, the nonce says nothing about the
//! code; the stream still counts the guesses.
//!
//! An invitation link works the same way ([`TokenDeriver`]): the stream keeps
//! the nonce, the link carries the token derived from it, and the stream is
//! keyed by the token's hash, so the store holds nothing a link could be
//! rebuilt from.

use hmac::{Hmac, Mac};
use rand::RngCore;
use sha2::{Digest, Sha256};

type HmacSha256 = Hmac<Sha256>;

/// How many bytes a nonce has: enough that two codes never share one.
pub const NONCE_LEN: usize = 16;

/// A fresh nonce for a code or a token.
pub fn new_nonce() -> Vec<u8> {
    let mut nonce = vec![0u8; NONCE_LEN];
    rand::rng().fill_bytes(&mut nonce);
    nonce
}

/// Derives the code a stream's nonce stands for, and checks a guess
/// against it. The code is bound to the tenant and the challenge, so a nonce
/// copied from one stream derives another code there.
pub trait CodeVerifier: Send + Sync {
    /// The six-digit code for `nonce` on this challenge.
    fn derive_code(&self, tenant: &str, challenge_id: &str, nonce: &[u8]) -> String;

    /// Whether `code` is the one `nonce` stands for. Constant time.
    fn verify(&self, tenant: &str, challenge_id: &str, nonce: &[u8], code: &str) -> bool;
}

/// Derives the token a mailed link carries from the nonce its stream keeps.
pub trait TokenDeriver: Send + Sync {
    /// The token for `nonce`: 43 characters of URL-safe base64, unguessable
    /// without the key.
    fn derive_token(&self, tenant: &str, nonce: &[u8]) -> String;
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
    /// `purpose` (`shopper-code`, `email-code`, `invitation`), so each
    /// purpose gets a key of its own from the same secret, apart from the
    /// sealing keys.
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

    fn mac(&self, label: &[u8], parts: &[&[u8]]) -> [u8; 32] {
        let mut mac = HmacSha256::new_from_slice(&self.key).expect("any key length works");
        mac.update(label);
        for part in parts {
            mac.update(&(part.len() as u32).to_be_bytes());
            mac.update(part);
        }
        mac.finalize().into_bytes().into()
    }
}

impl CodeVerifier for HmacCodeVerifier {
    fn derive_code(&self, tenant: &str, challenge_id: &str, nonce: &[u8]) -> String {
        let mac = self.mac(
            b"code",
            &[tenant.as_bytes(), challenge_id.as_bytes(), nonce],
        );
        // 2^64 mod 10^6 leaves a bias of one part in 10^13: even.
        let number = u64::from_be_bytes(mac[..8].try_into().expect("eight bytes"));
        format!("{:06}", number % 1_000_000)
    }

    fn verify(&self, tenant: &str, challenge_id: &str, nonce: &[u8], code: &str) -> bool {
        constant_time_eq(
            self.derive_code(tenant, challenge_id, nonce).as_bytes(),
            code.trim().as_bytes(),
        )
    }
}

impl TokenDeriver for HmacCodeVerifier {
    fn derive_token(&self, tenant: &str, nonce: &[u8]) -> String {
        use base64::Engine;

        let mac = self.mac(b"token", &[tenant.as_bytes(), nonce]);
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(mac)
    }
}

/// Whether the two are equal, in a time that depends on their lengths
/// only.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b) {
        diff |= x ^ y;
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_code_is_six_digits_bound_to_its_key_tenant_and_challenge() {
        let verifier = HmacCodeVerifier::new([7; 32]);
        let nonce = new_nonce();
        assert_eq!(nonce.len(), NONCE_LEN);
        assert_ne!(nonce, new_nonce());
        let code = verifier.derive_code("acme", "challenge-1", &nonce);
        assert_eq!(code.len(), 6);
        assert!(code.bytes().all(|b| b.is_ascii_digit()), "{code}");
        assert_eq!(
            verifier.derive_code("acme", "challenge-1", &nonce),
            code,
            "the same nonce derives the same code, on any node"
        );
        assert!(verifier.verify("acme", "challenge-1", &nonce, &code));
        assert!(verifier.verify("acme", "challenge-1", &nonce, &format!(" {code} ")));
        let wrong = format!("{:06}", (code.parse::<u32>().unwrap() + 1) % 1_000_000);
        assert!(!verifier.verify("acme", "challenge-1", &nonce, &wrong));
        assert!(!verifier.verify("acme", "challenge-1", &nonce, ""));
        assert!(!verifier.verify("acme", "challenge-1", &nonce, &code[..5]));
        assert_ne!(verifier.derive_code("other", "challenge-1", &nonce), code);
        assert_ne!(verifier.derive_code("acme", "challenge-2", &nonce), code);
        assert_ne!(
            HmacCodeVerifier::new([8; 32]).derive_code("acme", "challenge-1", &nonce),
            code,
            "another key, another code"
        );
    }

    #[test]
    fn codes_spread_over_the_million() {
        let verifier = HmacCodeVerifier::new([7; 32]);
        let codes: std::collections::BTreeSet<String> = (0..200)
            .map(|i| verifier.derive_code("", "c", &[i as u8, 1, 2]))
            .collect();
        assert!(codes.len() > 190, "{}", codes.len());
        assert!(codes.iter().any(|code| code.starts_with('0')));
    }

    #[test]
    fn a_purpose_derives_its_own_key() {
        let shoppers = HmacCodeVerifier::derive("shopper-code", "a-secret");
        let email = HmacCodeVerifier::derive("email-code", "a-secret");
        let nonce = new_nonce();
        let code = shoppers.derive_code("", "c", &nonce);
        assert!(shoppers.verify("", "c", &nonce, &code));
        assert_ne!(email.derive_code("", "c", &nonce), code);
        assert_ne!(
            HmacCodeVerifier::derive("shopper-code", "another").derive_code("", "c", &nonce),
            code
        );
    }

    #[test]
    fn a_token_is_a_url_safe_mac_of_the_nonce() {
        let invitations = HmacCodeVerifier::derive("invitation", "a-secret");
        let nonce = new_nonce();
        let token = invitations.derive_token("", &nonce);
        assert_eq!(token.len(), 43, "{token}");
        assert!(
            token
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'),
            "{token}"
        );
        assert_eq!(invitations.derive_token("", &nonce), token);
        assert_ne!(invitations.derive_token("acme", &nonce), token);
        assert_ne!(invitations.derive_token("", &new_nonce()), token);
        assert_ne!(
            HmacCodeVerifier::derive("invitation", "another").derive_token("", &nonce),
            token
        );
        // A code and a token under the same key share nothing.
        assert_ne!(
            invitations.derive_token("", &nonce).as_bytes()[..6],
            invitations.derive_code("", "", &nonce).as_bytes()[..6]
        );
    }

    #[test]
    fn equality_is_checked_whole() {
        assert!(constant_time_eq(b"123456", b"123456"));
        assert!(!constant_time_eq(b"123456", b"123457"));
        assert!(!constant_time_eq(b"123456", b"12345"));
        assert!(constant_time_eq(b"", b""));
    }
}
