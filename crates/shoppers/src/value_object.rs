use timada_orders::ShippingAddress;

use timada_core::Key as ShopperKey;

/// How long a mailed code stays valid, in seconds.
pub const CODE_TTL_SECS: u64 = 10 * 60;

/// How long after a code is mailed another one may be asked for the same
/// address, in seconds.
pub const CODE_RESEND_AFTER_SECS: u64 = 60;

/// Wrong guesses after which a code is dead and a new one must be mailed.
pub const CODE_MAX_ATTEMPTS: u32 = 5;

/// How long a sign-in lasts, in seconds.
pub const SESSION_TTL_SECS: u64 = 30 * 24 * 60 * 60;

/// The most addresses one shopper keeps.
pub const MAX_ADDRESSES: usize = 10;

/// Aggregate id of the shopper behind a normalized address. It is a plain
/// hash: anyone holding a candidate address can test it, even once the
/// shopper is forgotten (the shop's orders keep the address anyway).
pub fn shopper_id(email: &str) -> String {
    evento::hash_ids(vec!["shopper", email])
}

/// Aggregate id of the codes mailed to a normalized address.
pub fn challenge_id(email: &str) -> String {
    evento::hash_ids(vec!["shopper-challenge", email])
}

/// What the challenge stream stores instead of a code. A six-digit code is
/// guessable under any hash; the attempt cap and the expiry protect it, the
/// hash only keeps the plaintext out of the store.
pub fn code_hash(challenge_id: &str, code: &str) -> String {
    evento::hash_ids(vec!["shopper-code", challenge_id, code])
}

/// Aggregate id of the session a token opens. Anyone reading the store sees
/// only this, never the token.
pub fn session_id(token: &str) -> String {
    evento::hash_ids(vec!["shopper-session", token])
}

/// The id of a saved address, from its (normalized) fields keyed by the
/// shopper's key: the same address saved twice names the same entry, and
/// once the key is destroyed the id says nothing of the address.
pub fn address_id(key: &ShopperKey, address: &ShippingAddress) -> String {
    use base64::Engine;
    use hmac::{Hmac, Mac};

    let mut mac = Hmac::<sha2::Sha256>::new_from_slice(key.as_bytes())
        .expect("HMAC takes a key of any length");
    mac.update(b"timada-shoppers/address");
    for field in [
        address.name.as_str(),
        &address.line1,
        address.line2.as_deref().unwrap_or_default(),
        &address.postal_code,
        &address.city,
        &address.country,
        address.phone.as_deref().unwrap_or_default(),
    ] {
        mac.update(&(field.len() as u32).to_be_bytes());
        mac.update(field.as_bytes());
    }
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes())
}

/// A uniformly random six-digit code, zero-padded.
pub(crate) fn generate_code() -> String {
    use rand::Rng;

    format!("{:06}", rand::rng().random_range(0..1_000_000u32))
}

/// A fresh session token: 32 random bytes as URL-safe base64 without padding
/// (43 characters).
pub(crate) fn generate_token() -> String {
    use base64::Engine;
    use rand::RngCore;

    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_are_six_digits() {
        for _ in 0..100 {
            let code = generate_code();
            assert_eq!(code.len(), 6);
            assert!(code.bytes().all(|b| b.is_ascii_digit()));
        }
    }

    #[test]
    fn tokens_differ() {
        assert_eq!(generate_token().len(), 43);
        assert_ne!(generate_token(), generate_token());
    }
}
