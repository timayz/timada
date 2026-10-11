use serde::Deserialize;
use timada_identity::{RegisterPasskey, RegistrationMode, VerifiedPasskey};
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{content::Json, error::bad_request, route},
};
use webauthn_rs::prelude::{RegisterPublicKeyCredential, Webauthn};

use crate::app::auth::passkey::{Done, into_error};
use crate::app::auth::{account_url, home_url};
use crate::auth::{
    cookies::{is_fresh, passkey_registration_flow},
    current_user, identity, login_context, now, start_session,
    webauthn::{encode_credential_id, passkey_to_json},
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
        Ok(None) | Err(_) => return Err(bad_request("this sign-in attempt expired").into()),
    };
    if !is_fresh(flow.issued_at, now()) {
        return Err(bad_request("this sign-in attempt expired").into());
    }
    let Some(mode) = RegistrationMode::parse(&flow.mode) else {
        return Err(bad_request("unknown mode").into());
    };

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

    let outcome = identity(cx)
        .register_passkey(RegisterPasskey {
            mode,
            session_user_id: current_user(cx).await.map(|u| u.id.clone()),
            bound_user_id: flow.bound_user_id,
            passkey: VerifiedPasskey {
                credential_id: encode_credential_id(passkey.cred_id()),
                passkey_json: passkey_to_json(&passkey).map_err(topcoat::Error::from_anyhow)?,
                // The `Passkey` type keeps attestation internals private.
                aaguid: String::new(),
                rp_id: webauthn
                    .get_allowed_origins()
                    .first()
                    .and_then(|origin| origin.host_str())
                    .unwrap_or_default()
                    .to_owned(),
                name,
            },
            account: flow.account,
            label: flow.label,
            context: login_context(cx),
        })
        .await
        .map_err(into_error)?;

    if outcome.created {
        start_session(cx, &outcome.user_id).await?;
        Ok(Json(Done {
            location: home_url(cx),
        }))
    } else {
        Ok(Json(Done {
            location: account_url(cx, None, Some("Passkey added.")),
        }))
    }
}
