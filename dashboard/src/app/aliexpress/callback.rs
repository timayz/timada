use std::sync::Arc;

use timada_tenancy::AuthorizeAliexpress;
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{
        error::{SeeOther, see_other},
        query_params, route,
    },
};

use crate::aliexpress::Aliexpress;
use crate::app::companies::company_id::company_url;
use crate::app::tenants::{error_code, tenancy};
use crate::auth::cookies::{aliexpress_flow, is_fresh};
use crate::auth::require_user;

#[topcoat::router::query_params(error = bad_request)]
struct CallbackQuery {
    code: Option<String>,
    state: Option<String>,
}

/// Where AliExpress sends the owner back: exchanges the code for the
/// account's tokens and keeps them sealed on the company the flow cookie
/// names.
#[route(GET)]
pub async fn callback(cx: &Cx) -> Result<SeeOther> {
    let user = require_user(cx).await?;
    let query = query_params::<CallbackQuery>(cx)?;
    // One shot: whatever happens next, the flow cannot be replayed.
    let store = aliexpress_flow(cx);
    let flow = match store.parse() {
        Ok(Some(store)) => {
            let flow = store.get();
            store.remove();
            flow
        }
        Ok(None) | Err(_) => {
            return Ok(see_other(crate::app::companies::companies_url(
                cx,
                Some("aliexpress_connect_expired"),
                None,
            )));
        }
    };
    let refused = |code: &str| {
        Ok(see_other(company_url(
            cx,
            &flow.company_id,
            Some(code),
            None,
        )))
    };
    if flow.user_id != user.id
        || !is_fresh(flow.issued_at, crate::auth::now())
        || query.state.as_deref() != Some(flow.state.as_str())
    {
        return refused("aliexpress_connect_expired");
    }
    let Some(code) = query.code.clone().filter(|code| !code.is_empty()) else {
        return refused("aliexpress_connect_refused");
    };
    let aliexpress = app_context::<Arc<Aliexpress>>(cx);
    if !aliexpress.enabled() {
        return refused("aliexpress_disabled");
    }
    let client = aliexpress
        .app_client()
        .map_err(topcoat::Error::from_anyhow)?;
    let tokens = match client.create_token(&code).await {
        Ok(tokens) => tokens,
        Err(err) => {
            tracing::warn!(company = %flow.company_id, %err, "AliExpress refused the code");
            return refused("aliexpress_connect_refused");
        }
    };
    let now = crate::auth::now();
    let seal = |plain: &str| aliexpress.seal(plain).map_err(topcoat::Error::from_anyhow);
    let authorized = tenancy(cx)
        .authorize_aliexpress(
            &user.id,
            &flow.company_id,
            AuthorizeAliexpress {
                access_token_sealed: seal(&tokens.access_token)?,
                refresh_token_sealed: seal(&tokens.refresh_token)?,
                expires_at: now + tokens.expires_in,
                refresh_expires_at: now + tokens.refresh_expires_in,
                account: tokens.account.chars().take(120).collect(),
            },
        )
        .await;
    match authorized {
        Ok(()) => Ok(see_other(company_url(
            cx,
            &flow.company_id,
            None,
            Some("AliExpress account connected."),
        ))),
        Err(err) if err.is_user_facing() => refused(error_code(&err)),
        Err(err) => Err(err.into()),
    }
}
