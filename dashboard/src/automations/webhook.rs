//! The webhook step: an address of the merchant's, kept sealed in the
//! scenario, posted what started a run, signed under a secret the step's
//! own (derived from the operator's secret: nothing is stored).
//!
//! The request is a JSON `POST` with `Timada-Signature: t={unix},v1={hex}`
//! (HMAC-SHA256 of `"{t}.{body}"` under the step's secret, as Stripe signs
//! its webhooks) and `Idempotency-Key: {run}:{step}`, the same on every try.
//! It carries ids, no personal data: the merchant's own app reads the
//! details from the store with its key.

use base64::Engine;
use hmac::{Hmac, Mac};
use sha2::Sha256;

use crate::seal::WebhookKeys;

/// What binds a sealed address to its tenant, scenario and step: one copied
/// into another step does not open.
fn binding(tenant_id: &str, scenario_id: &str, step_id: &str) -> String {
    format!("{tenant_id}\n{scenario_id}\n{step_id}\n")
}

/// The address, sealed for the definition (base64).
pub fn seal_url(
    keys: &WebhookKeys,
    tenant_id: &str,
    scenario_id: &str,
    step_id: &str,
    url: &str,
) -> anyhow::Result<String> {
    let plain = format!("{}{url}", binding(tenant_id, scenario_id, step_id));
    Ok(base64::engine::general_purpose::STANDARD.encode(keys.sealer.seal(&plain)?))
}

/// The address a definition holds, opened; `None` for one sealed for
/// another step, or under another secret.
pub fn open_url(
    keys: &WebhookKeys,
    tenant_id: &str,
    scenario_id: &str,
    step_id: &str,
    sealed: &str,
) -> Option<String> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(sealed)
        .ok()?;
    let plain = keys.sealer.open(&bytes).ok()?;
    plain
        .strip_prefix(&binding(tenant_id, scenario_id, step_id))
        .map(str::to_owned)
}

/// The step's signing secret, as the merchant's app checks it with.
pub fn signing_secret(
    keys: &WebhookKeys,
    tenant_id: &str,
    scenario_id: &str,
    step_id: &str,
) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(&keys.signing).expect("HMAC takes any key length");
    mac.update(binding(tenant_id, scenario_id, step_id).as_bytes());
    let hex: String = mac
        .finalize()
        .into_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    format!("whsec_{hex}")
}

/// The `Timada-Signature` header of `body` sent at `timestamp`.
pub fn signature(secret: &str, body: &[u8], timestamp: u64) -> String {
    let mut mac =
        Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("HMAC takes any key length");
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

/// How a call went.
pub enum Delivery {
    /// The endpoint took it (2xx).
    Taken(u16),
    /// Worth trying again: no answer, a timeout, 408, 429 or a 5xx.
    Again(String),
    /// Not worth trying again: another 4xx, or an address refused.
    Refused(String),
}

/// Posts `body` to `url`, signed. A host that is not public is refused
/// unless `allow_private_hosts` (development and tests).
pub async fn deliver(
    client: &reqwest::Client,
    url: &str,
    body: Vec<u8>,
    signature: String,
    idempotency_key: &str,
    allow_private_hosts: bool,
) -> Delivery {
    if !allow_private_hosts && let Err(err) = crate::net::check_public_url(url).await {
        return Delivery::Refused(err.to_string());
    }
    let sent = client
        .post(url)
        .header("content-type", "application/json")
        .header("timada-signature", signature)
        .header("idempotency-key", idempotency_key)
        .body(body)
        .send()
        .await;
    match sent {
        Ok(response) => {
            let status = response.status().as_u16();
            match status {
                200..=299 => Delivery::Taken(status),
                408 | 429 | 500..=599 => {
                    Delivery::Again(format!("The endpoint answered {status}."))
                }
                _ => Delivery::Refused(format!("The endpoint answered {status}.")),
            }
        }
        Err(err) => Delivery::Again(format!("The endpoint could not be reached: {err}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys() -> WebhookKeys {
        WebhookKeys {
            sealer: crate::seal::Sealer::new(
                "automation-webhook",
                "a-secrets-key-of-thirty-two-chars!!",
            ),
            signing: [3; 32],
        }
    }

    #[test]
    fn an_address_opens_for_its_own_step_only() {
        let keys = keys();
        let sealed = seal_url(&keys, "t1", "sc1", "s1", "https://hooks.example/abc").unwrap();
        assert!(!sealed.contains("hooks.example"));
        assert_eq!(
            open_url(&keys, "t1", "sc1", "s1", &sealed).as_deref(),
            Some("https://hooks.example/abc")
        );
        assert_eq!(open_url(&keys, "t1", "sc1", "s2", &sealed), None);
        assert_eq!(open_url(&keys, "t2", "sc1", "s1", &sealed), None);
    }

    #[test]
    fn a_step_signs_with_a_secret_of_its_own() {
        let keys = keys();
        let one = signing_secret(&keys, "t1", "sc1", "s1");
        assert!(one.starts_with("whsec_"));
        assert_ne!(one, signing_secret(&keys, "t1", "sc1", "s2"));
        assert_eq!(one, signing_secret(&keys, "t1", "sc1", "s1"));
        let header = signature(&one, b"{}", 1_700_000_000);
        // Checked the way the payments webhooks are.
        assert!(crate::payments::signature::verify(
            b"{}",
            &header,
            &one,
            1_700_000_000
        ));
    }
}
