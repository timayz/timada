use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{content::Json, error::bad_request, route},
};
use webauthn_rs::prelude::{RequestChallengeResponse, Webauthn};

use crate::auth::{
    cookies::{PasskeyLoginFlow, passkey_login_flow},
    current_user, now,
};

/// A usernameless challenge: the authenticator picks the credential.
#[route(POST)]
pub async fn start(cx: &Cx) -> Result<Json<RequestChallengeResponse>> {
    if current_user(cx).await.is_some() {
        return Err(bad_request("already signed in").into());
    }
    let webauthn = app_context::<Webauthn>(cx);
    let (challenge, state) = webauthn.start_discoverable_authentication()?;
    passkey_login_flow(cx)
        .set(PasskeyLoginFlow {
            state,
            issued_at: now(),
        })
        .commit()?;
    Ok(Json(challenge))
}
