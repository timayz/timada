use serde::Deserialize;
use timada_identity::{AddPasskey, VerifiedPasskey};
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{content::Json, error::bad_request, route},
};
use webauthn_rs::prelude::{RegisterPublicKeyCredential, Webauthn};

use crate::app::auth::account_url;
use crate::app::auth::passkey::{Done, into_error};
use crate::auth::{
    cookies::{is_fresh, passkey_registration_flow},
    current_user, identity, now,
    webauthn::{aaguid_from_attestation, encode_credential_id, passkey_to_json},
};

#[derive(Deserialize)]
struct FinishRequest {
    /// The browser's `PublicKeyCredential.toJSON()`.
    credential: RegisterPublicKeyCredential,
    /// Display name for the passkey.
    name: Option<String>,
}

#[route(POST)]
pub async fn finish(cx: &Cx, Json(req): Json<FinishRequest>) -> Result<Json<Done>> {
    let webauthn = app_context::<Webauthn>(cx);
    let store = passkey_registration_flow(cx);
    let flow = match store.parse() {
        Ok(Some(store)) => {
            let flow = store.get();
            store.remove();
            flow
        }
        Ok(None) | Err(_) => return Err(bad_request("this attempt expired").into()),
    };
    if !is_fresh(flow.issued_at, now()) {
        return Err(bad_request("this attempt expired").into());
    }

    let passkey = webauthn
        .finish_passkey_registration(&req.credential, &flow.state)
        .map_err(|err| {
            tracing::warn!(%err, "passkey registration failed");
            bad_request("the passkey could not be verified")
        })?;
    let name = req
        .name
        .as_deref()
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .map(|n| n.chars().take(64).collect())
        .unwrap_or_else(|| "Passkey".to_owned());

    identity(cx)
        .add_passkey(AddPasskey {
            session_user_id: current_user(cx).await.map(|u| u.id.clone()),
            bound_user_id: flow.bound_user_id,
            passkey: VerifiedPasskey {
                credential_id: encode_credential_id(passkey.cred_id()),
                passkey_json: passkey_to_json(&passkey).map_err(topcoat::Error::from_anyhow)?,
                // The `Passkey` type keeps attestation internals private, so
                // read the authenticator model id off the raw response.
                aaguid: aaguid_from_attestation(
                    req.credential.response.attestation_object.as_ref(),
                )
                .map(|id| id.hyphenated().to_string())
                .unwrap_or_default(),
                rp_id: webauthn
                    .get_allowed_origins()
                    .first()
                    .and_then(|origin| origin.host_str())
                    .unwrap_or_default()
                    .to_owned(),
                name,
            },
        })
        .await
        .map_err(into_error)?;

    Ok(Json(Done {
        location: account_url(cx, None, Some("Passkey added.")),
    }))
}
