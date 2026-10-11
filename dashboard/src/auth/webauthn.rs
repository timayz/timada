//! Passkeys through webauthn-rs. The relying party is the host of
//! `server.url`; credentials are discoverable so sign-in needs no username.

use anyhow::{Context, Result};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use uuid::Uuid;
use webauthn_rs::prelude::{CredentialID, Passkey, Url, Webauthn, WebauthnBuilder};

use crate::config::AuthConfig;

pub fn build(config: &AuthConfig, server_url: &str) -> Result<Webauthn> {
    let origin = Url::parse(server_url).context("server.url is not a valid URL")?;
    let rp_id = origin
        .host_str()
        .context("server.url has no host")?
        .to_owned();
    WebauthnBuilder::new(&rp_id, &origin)
        .context("cannot configure passkeys for server.url")?
        .rp_name(&config.rp_name)
        .build()
        .context("cannot configure passkeys")
}

/// The WebAuthn user handle for a user: stable per user, so an authenticator
/// replaces rather than duplicates a credential for the same account.
pub fn user_handle(user_id: &str) -> Uuid {
    Uuid::new_v5(
        &Uuid::NAMESPACE_URL,
        format!("timada:user:{user_id}").as_bytes(),
    )
}

/// Credential ids are stored as base64url without padding, the form the
/// browser uses in `PublicKeyCredential.id`.
pub fn encode_credential_id(id: &CredentialID) -> String {
    URL_SAFE_NO_PAD.encode(id.as_ref())
}

pub fn decode_credential_id(id: &str) -> Option<CredentialID> {
    URL_SAFE_NO_PAD.decode(id).ok().map(CredentialID::from)
}

pub fn passkey_to_json(passkey: &Passkey) -> Result<String> {
    serde_json::to_string(passkey).context("cannot serialize the passkey")
}

pub fn passkey_from_json(json: &str) -> Result<Passkey> {
    serde_json::from_str(json).context("stored passkey is unreadable")
}

/// The authenticator model id (AAGUID) of a freshly registered credential.
/// webauthn-rs verifies the attestation object but keeps the parsed authData
/// private, so read it off the raw CBOR: `authData` is `rpIdHash(32) ||
/// flags(1) || signCount(4) || attestedCredentialData`, whose first 16 bytes
/// are the AAGUID when the AT flag (bit 6) is set. `None` for a malformed
/// object or one without credential data; all zeros is a valid "anonymous"
/// answer some browsers give.
pub fn aaguid_from_attestation(attestation_object: &[u8]) -> Option<Uuid> {
    const ATTESTED_CREDENTIAL_DATA: u8 = 0x40;
    let value: ciborium::Value = ciborium::from_reader(attestation_object).ok()?;
    let entries = value.into_map().ok()?;
    let auth_data = entries
        .into_iter()
        .find_map(|(key, value)| (key.as_text() == Some("authData")).then_some(value))?;
    let auth_data = auth_data.into_bytes().ok()?;
    if auth_data.len() < 53 || auth_data[32] & ATTESTED_CREDENTIAL_DATA == 0 {
        return None;
    }
    Uuid::from_slice(&auth_data[37..53]).ok()
}

/// Well-known passkey providers, from the community-maintained list at
/// <https://github.com/passkeydeveloper/passkey-authenticator-aaguids>
/// (fetched 2026-10-11). Security keys are left out: the user names those.
pub fn authenticator_name(aaguid: &str) -> Option<&'static str> {
    Some(match aaguid {
        "fbfc3007-154e-4ecc-8c0b-6e020557d7bd" => "Apple Passwords",
        "dd4ec289-e01d-41c9-bb89-70fa845d4bf2" => "iCloud Keychain (Managed)",
        "ea9b8d66-4d01-1d21-3ce4-b6b48cb575d4" => "Google Password Manager",
        "adce0002-35bc-c60a-648b-0b25f1f05503" => "Chrome on Mac",
        "b5397666-4885-aa6b-cebf-e52262a439a2" => "Chromium Browser",
        "771b48fd-d3d4-4f74-9232-fc157ab0507a" => "Edge on Mac",
        "08987058-cadc-4b81-b6e1-30de50dcbe96"
        | "9ddd1817-af5a-4672-a2b9-3e3dd95000a9"
        | "6028b017-b1d4-4c02-b4b3-afcdafc96bb2" => "Windows Hello",
        "d3452668-01fd-4c12-926c-83a4204853aa" => "Microsoft Password Manager",
        "53414d53-554e-4700-0000-000000000000" => "Samsung Pass",
        "bada5566-a7aa-401f-bd96-45619a55120d" => "1Password",
        "d548826e-79b4-db40-a3d8-11116f7e8349" => "Bitwarden",
        "531126d6-e717-415c-9320-3d9aa6981239" => "Dashlane",
        "f3809540-7f14-49c1-a8b3-8f813b225541" => "Enpass",
        "fdb141b2-5d84-443e-8a35-4698c205a502" => "KeePassXC",
        "eaecdef2-1c31-5634-8639-f1cbd9c00a08" => "KeePassDX",
        "0ea242b4-43c4-4a1b-8b17-dd6d0b6baec6" => "Keeper",
        "b78a0a55-6ef8-d246-a042-ba0f6d55050c" => "LastPass",
        "b84e4048-15dc-4dd0-8640-f4f60813c8af" => "NordPass",
        "50726f74-6f6e-5061-7373-50726f746f6e" => "Proton Pass",
        "a10c6dd9-465e-4226-8198-c7c44b91c555" => "Kaspersky Password Manager",
        "fa37f553-f9b6-4adb-ac53-8bbb57ebdf0d" => "Norton Password Manager",
        "39a5647e-1853-446c-a1f6-a79bae9f5bc7" => "IDmelon",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal attestation object: `{"fmt": "none", "attStmt": {}, "authData": <bytes>}`.
    fn attestation_object(auth_data: &[u8]) -> Vec<u8> {
        let value = ciborium::Value::Map(vec![
            (
                ciborium::Value::Text("fmt".into()),
                ciborium::Value::Text("none".into()),
            ),
            (
                ciborium::Value::Text("attStmt".into()),
                ciborium::Value::Map(vec![]),
            ),
            (
                ciborium::Value::Text("authData".into()),
                ciborium::Value::Bytes(auth_data.to_vec()),
            ),
        ]);
        let mut out = Vec::new();
        ciborium::into_writer(&value, &mut out).unwrap();
        out
    }

    fn auth_data(flags: u8, aaguid: [u8; 16]) -> Vec<u8> {
        let mut data = vec![0u8; 32]; // rpIdHash
        data.push(flags);
        data.extend_from_slice(&[0, 0, 0, 1]); // signCount
        data.extend_from_slice(&aaguid);
        data.extend_from_slice(&[0, 2, 0xaa, 0xbb]); // credentialIdLength + id
        data
    }

    #[test]
    fn reads_the_aaguid_when_credential_data_is_attested() {
        let aaguid = Uuid::parse_str("ea9b8d66-4d01-1d21-3ce4-b6b48cb575d4").unwrap();
        let object = attestation_object(&auth_data(0x45, *aaguid.as_bytes()));
        assert_eq!(aaguid_from_attestation(&object), Some(aaguid));
    }

    #[test]
    fn rejects_missing_credential_data_and_garbage() {
        let object = attestation_object(&auth_data(0x05, [7; 16]));
        assert_eq!(aaguid_from_attestation(&object), None, "AT flag unset");
        let object = attestation_object(&[0u8; 40]);
        assert_eq!(aaguid_from_attestation(&object), None, "too short");
        assert_eq!(aaguid_from_attestation(b"not cbor"), None);
        assert_eq!(aaguid_from_attestation(&[]), None);
    }

    #[test]
    fn names_known_providers_only() {
        assert_eq!(
            authenticator_name("d548826e-79b4-db40-a3d8-11116f7e8349"),
            Some("Bitwarden")
        );
        assert_eq!(authenticator_name(""), None);
        assert_eq!(
            authenticator_name("00000000-0000-0000-0000-000000000000"),
            None
        );
    }
}
