use serde::Deserialize;
use timada_identity::{MAX_PASSKEYS_PER_USER, RegistrationMode};
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{
        content::Json,
        error::{bad_request, forbidden},
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
    mode: String,
    /// For `add_to_user`: the user the page was rendered for.
    user_id: Option<String>,
    /// For `create_user`: the new user's name.
    name: Option<String>,
}

#[route(POST)]
pub async fn start(
    cx: &Cx,
    Json(req): Json<StartRequest>,
) -> Result<Json<CreationChallengeResponse>> {
    let webauthn = app_context::<Webauthn>(cx);
    let Some(mode) = RegistrationMode::parse(&req.mode) else {
        return Err(bad_request("unknown mode").into());
    };
    let user = current_user(cx).await;

    let (bound_user_id, account, label, exclude) = match (mode, user) {
        (RegistrationMode::AddToUser, Some(user)) => {
            if req.user_id.as_deref() != Some(user.id.as_str()) {
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
            (
                user.id.clone(),
                user.account.clone(),
                user.label.clone(),
                Some(exclude),
            )
        }
        (RegistrationMode::AddToUser, None) => {
            return Err(topcoat::router::error::unauthorized().into());
        }
        (RegistrationMode::CreateUser, Some(_)) => {
            return Err(bad_request("already signed in").into());
        }
        (RegistrationMode::CreateUser, None) => {
            let name = req.name.as_deref().map(str::trim).unwrap_or_default();
            if name.is_empty() || name.chars().count() > 64 {
                return Err(bad_request("please enter a name (up to 64 characters)").into());
            }
            (
                ulid::Ulid::generate().to_string(),
                name.to_owned(),
                name.to_owned(),
                None,
            )
        }
    };

    let (mut challenge, state) = webauthn.start_passkey_registration(
        user_handle(&bound_user_id),
        &account,
        &label,
        exclude,
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
            mode: mode.as_str().to_owned(),
            bound_user_id,
            account,
            label,
            state,
            issued_at: now(),
        })
        .commit()?;
    Ok(Json(challenge))
}
