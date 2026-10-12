use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{content::Json, route},
};
use webauthn_rs::prelude::{RequestChallengeResponse, Webauthn};

use crate::auth::{
    cookies::{PasskeyLoginFlow, passkey_login_flow},
    now,
};

/// A usernameless challenge: the authenticator picks the credential. Allowed
/// while signed in: finishing with another user's passkey switches accounts.
#[route(POST)]
pub async fn start(cx: &Cx) -> Result<Json<RequestChallengeResponse>> {
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
