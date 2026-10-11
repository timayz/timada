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
