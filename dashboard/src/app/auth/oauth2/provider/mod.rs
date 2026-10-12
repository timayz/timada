//! `/auth/oauth2/{provider}`: starting a flow (POST) and its callback.

pub mod callback;

use serde::Deserialize;
use timada_identity::OAuth2Mode;
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{
        content::Form,
        error::{SeeOther, bad_request, not_found, see_other},
        module_param, path_param, route,
    },
};

use crate::app::auth::{account_url, login_url};
use crate::auth::{
    cookies::{OidcFlow, oidc_flow},
    current_user, now,
    oidc::Providers,
};

module_param!(pub provider);

#[derive(Deserialize)]
struct StartForm {
    mode: String,
    /// For `add_to_user`: the user the account page was rendered for.
    user_id: Option<String>,
}

/// Sends the browser to the provider. A POST so the router's origin policy
/// applies and nothing can start a flow by following a link.
#[route(POST)]
pub async fn start(cx: &Cx, Form(form): Form<StartForm>) -> Result<SeeOther> {
    let name = path_param::<Provider>(cx);
    let providers = app_context::<Providers>(cx);
    let Some(provider) = providers.get(name) else {
        return Err(not_found().into());
    };
    let Some(mode) = OAuth2Mode::parse(&form.mode) else {
        return Err(bad_request("unknown mode").into());
    };

    // Login modes run whatever the session: signing in to another account
    // switches, and the domain refuses creating one while signed in.
    let user = current_user(cx).await;
    let bound_user_id = match (mode, user) {
        (OAuth2Mode::AddToUser, Some(user))
            if form.user_id.as_deref() == Some(user.id.as_str()) =>
        {
            Some(user.id.clone())
        }
        (OAuth2Mode::AddToUser, Some(_)) => {
            return Ok(see_other(account_url(cx, Some("session_changed"), None)));
        }
        (OAuth2Mode::AddToUser, None) => {
            return Ok(see_other(login_url(cx, Some("sign_in_required"))));
        }
        (_, _) => None,
    };

    let authorization = providers.authorize(provider);
    oidc_flow(cx)
        .set(OidcFlow {
            provider: provider.name.clone(),
            mode: mode.as_str().to_owned(),
            state: authorization.state,
            nonce: authorization.nonce,
            pkce_verifier: authorization.pkce_verifier,
            bound_user_id,
            issued_at: now(),
        })
        .commit()?;
    Ok(see_other(authorization.url))
}
