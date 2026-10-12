use serde::Deserialize;
use timada_identity::MAX_PASSKEYS_PER_USER;
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{
        content::Json,
        error::{bad_request, forbidden, unauthorized},
        route,
    },
};
use webauthn_rs::prelude::{CreationChallengeResponse, Webauthn};
use webauthn_rs_proto::ResidentKeyRequirement;

use crate::auth::{
    cookies::{PasskeyRegistrationFlow, passkey_registration_flow},
    current_user, now,
    webauthn::{decode_credential_id, user_handle},
};

#[derive(Deserialize)]
struct StartRequest {
    /// The user the page was rendered for; must be the signed-in one.
    user_id: String,
}

/// Starts adding a passkey to the signed-in user's account.
#[route(POST)]
pub async fn start(
    cx: &Cx,
    Json(req): Json<StartRequest>,
) -> Result<Json<CreationChallengeResponse>> {
    let webauthn = app_context::<Webauthn>(cx);
    let Some(user) = current_user(cx).await else {
        return Err(unauthorized().into());
    };
    if req.user_id != user.id {
        return Err(forbidden().into());
    }
    if user.passkeys.len() >= MAX_PASSKEYS_PER_USER {
        return Err(bad_request("this account has reached its passkey limit").into());
    }
    let exclude: Vec<_> = user
        .passkeys
        .iter()
        .filter_map(|p| decode_credential_id(&p.credential_id))
        .collect();

    let (mut challenge, state) = webauthn.start_passkey_registration(
        user_handle(&user.id),
        &user.account,
        &user.label,
        Some(exclude),
    )?;
    // webauthn-rs leaves residency "discouraged" so that storage-limited
    // security keys can register. Sign-in here is usernameless, which needs a
    // discoverable credential, so ask for one up front: a key that cannot
    // store it fails at registration with a clear browser message instead of
    // at its first sign-in.
    if let Some(selection) = challenge.public_key.authenticator_selection.as_mut() {
        selection.resident_key = Some(ResidentKeyRequirement::Required);
        selection.require_resident_key = true;
    }
    passkey_registration_flow(cx)
        .set(PasskeyRegistrationFlow {
            bound_user_id: user.id.clone(),
            state,
            issued_at: now(),
        })
        .commit()?;
    Ok(Json(challenge))
}
