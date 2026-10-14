//! Stripe's webhook signature: the `Stripe-Signature` header carries a
//! timestamp `t` and one or more `v1` HMAC-SHA256 of `"{t}.{body}"` under
//! the endpoint's signing secret. Checked here rather than through a parser
//! of the whole event, so a payload in a newer API version than this build
//! knows still verifies: the handler only reads the event's type and the
//! object's id, then asks Stripe for the object itself.

use hmac::{Hmac, Mac};
use sha2::Sha256;

/// How far a signature's timestamp may be from now, either way: Stripe's
/// own default tolerance.
pub const TOLERANCE_SECS: i64 = 300;

/// Whether `header` signs `body` under `secret` at a time close to `now`.
pub fn verify(body: &[u8], header: &str, secret: &str, now: i64) -> bool {
    let mut timestamp = None;
    let mut signatures = Vec::new();
    for part in header.split(',') {
        match part.trim().split_once('=') {
            Some(("t", value)) => timestamp = value.parse::<i64>().ok(),
            Some(("v1", value)) => signatures.extend(decode_hex(value)),
            _ => {}
        }
    }
    let Some(timestamp) = timestamp else {
        return false;
    };
    if (now - timestamp).abs() > TOLERANCE_SECS {
        return false;
    }
    signatures.iter().any(|signature| {
        let Ok(mut mac) = Hmac::<Sha256>::new_from_slice(secret.as_bytes()) else {
            return false;
        };
        mac.update(timestamp.to_string().as_bytes());
        mac.update(b".");
        mac.update(body);
        mac.verify_slice(signature).is_ok()
    })
}

/// The header Stripe would send for `body` at `timestamp`; for tests and
/// local tools.
#[cfg(test)]
pub fn sign(body: &[u8], secret: &str, timestamp: i64) -> String {
    // HMAC takes a key of any length: this cannot fail.
    let Ok(mut mac) = Hmac::<Sha256>::new_from_slice(secret.as_bytes()) else {
        return String::new();
    };
    mac.update(timestamp.to_string().as_bytes());
    mac.update(b".");
    mac.update(body);
    let hex: String = mac
        .finalize()
        .into_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    format!("t={timestamp},v1={hex}")
}

fn decode_hex(value: &str) -> Option<Vec<u8>> {
    if !value.len().is_multiple_of(2) {
        return None;
    }
    (0..value.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(value.get(i..i + 2)?, 16).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_signature_verifies_once_and_only_in_time() {
        let body = br#"{"id":"evt_1"}"#;
        let header = sign(body, "whsec_test", 1_000);
        assert!(verify(body, &header, "whsec_test", 1_010));
        assert!(!verify(body, &header, "whsec_other", 1_010));
        assert!(!verify(br#"{"id":"evt_2"}"#, &header, "whsec_test", 1_010));
        assert!(!verify(
            body,
            &header,
            "whsec_test",
            1_000 + TOLERANCE_SECS + 1
        ));
        assert!(!verify(body, "garbage", "whsec_test", 1_000));
        // Several v1 values (a secret being rolled): any one may match.
        let rolled = format!("{header},v1=00ff");
        assert!(verify(body, &rolled, "whsec_test", 1_000));
    }
}
