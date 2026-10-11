use timada_identity::{CompleteOAuth2, OAuth2Mode};
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{
        error::{SeeOther, bad_request, see_other},
        path_param, query_params, route,
    },
};

use super::Provider;
use crate::app::auth::{account_url, error_code, home_url, login_url};
use crate::auth::{
    cookies::{is_fresh, oidc_flow},
    current_user, identity, login_context, now,
    oidc::Providers,
    start_session,
};

#[topcoat::router::query_params(error = bad_request)]
struct CallbackQuery {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
    error_description: Option<String>,
}

/// The provider sends the browser back here. Nothing changes until the
/// `state` matches the one sealed in the flow cookie.
#[route(GET)]
pub async fn callback(cx: &Cx) -> Result<SeeOther> {
    let name = path_param::<Provider>(cx);
    let query = query_params::<CallbackQuery>(cx)?;
    let providers = app_context::<Providers>(cx);

    // One shot: whatever happens next, the flow cannot be replayed.
    let store = oidc_flow(cx);
    let flow = match store.parse() {
        Ok(Some(store)) => {
            let flow = store.get();
            store.remove();
            flow
        }
        Ok(None) | Err(_) => return Ok(see_other(login_url(cx, Some("expired")))),
    };

    let Some(provider) = providers.get(name) else {
        return Ok(see_other(login_url(cx, Some("unknown_provider"))));
    };
    if flow.provider != provider.name || !is_fresh(flow.issued_at, now()) {
        return Ok(see_other(login_url(cx, Some("expired"))));
    }
    let Some(mode) = OAuth2Mode::parse(&flow.mode) else {
        return Ok(see_other(login_url(cx, Some("failed"))));
    };
    let failure_url = |code: &str| match mode {
        OAuth2Mode::AddToUser => account_url(cx, Some(code), None),
        _ => login_url(cx, Some(code)),
    };

    if let Some(error) = &query.error {
        tracing::warn!(provider = %provider.name, error, description = ?query.error_description, "provider returned an error");
        return Ok(see_other(failure_url("provider")));
    }
    let (Some(code), Some(state)) = (query.code.clone(), query.state.as_deref()) else {
        return Err(bad_request("missing code or state").into());
    };
    if state != flow.state {
        tracing::warn!(provider = %provider.name, "OIDC state mismatch");
        return Ok(see_other(failure_url("expired")));
    }

    // The browser's session now, and the one the flow was bound to.
    let session_user_id = current_user(cx).await.map(|u| u.id.clone());
    if mode == OAuth2Mode::AddToUser && session_user_id != flow.bound_user_id {
        return Ok(see_other(account_url(cx, Some("session_changed"), None)));
    }

    let identity_claims = match providers
        .exchange(provider, code, flow.pkce_verifier, flow.nonce)
        .await
    {
        Ok(claims) => claims,
        Err(err) => {
            tracing::warn!(provider = %provider.name, error = %format!("{err:#}"), "token exchange failed");
            return Ok(see_other(failure_url("provider")));
        }
    };

    let outcome = identity(cx)
        .complete_oauth2(CompleteOAuth2 {
            mode,
            session_user_id,
            identity: identity_claims,
            context: login_context(cx),
        })
        .await;
    match outcome {
        Ok(outcome) if outcome.linked => {
            Ok(see_other(account_url(cx, None, Some("Provider linked."))))
        }
        Ok(outcome) => {
            start_session(cx, &outcome.user_id).await?;
            Ok(see_other(home_url(cx)))
        }
        Err(err) if err.is_user_facing() => Ok(see_other(failure_url(error_code(&err)))),
        Err(err) => Err(err.into()),
    }
}
