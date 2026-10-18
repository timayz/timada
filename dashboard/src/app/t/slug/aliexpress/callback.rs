use std::sync::Arc;

use timada_tenancy::{AuthorizeAliexpress, PERMISSION_ALIEXPRESS_MANAGE};
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{
        error::{SeeOther, see_other},
        query_params, route,
    },
};

use super::aliexpress_url;
use crate::aliexpress::Aliexpress;
use crate::app::t::slug::require_tenant;
use crate::app::tenants::{error_code, tenancy};
use crate::auth::cookies::{aliexpress_flow, is_fresh};

#[topcoat::router::query_params(error = bad_request)]
struct CallbackQuery {
    code: Option<String>,
    state: Option<String>,
}

/// Where AliExpress sends the merchant back: exchanges the code for the
/// account's tokens and keeps them sealed.
#[route(GET)]
pub async fn callback(cx: &Cx) -> Result<SeeOther> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_ALIEXPRESS_MANAGE)
        .await?;
    let slug = access.tenant.slug.as_str();
    let query = query_params::<CallbackQuery>(cx)?;
    let refused = |code: &str| Ok(see_other(aliexpress_url(cx, slug, Some(code), None)));
    // One shot: whatever happens next, the flow cannot be replayed.
    let store = aliexpress_flow(cx);
    let flow = match store.parse() {
        Ok(Some(store)) => {
            let flow = store.get();
            store.remove();
            flow
        }
        Ok(None) | Err(_) => return refused("aliexpress_connect_expired"),
    };
    if flow.tenant_id != access.tenant.id
        || flow.user_id != access.user_id
        || !is_fresh(flow.issued_at, crate::auth::now())
        || query.state.as_deref() != Some(flow.state.as_str())
    {
        return refused("aliexpress_connect_expired");
    }
    let Some(code) = query.code.clone().filter(|code| !code.is_empty()) else {
        return refused("aliexpress_connect_refused");
    };
    let Some(settings) = &access.tenant.aliexpress else {
        return refused("aliexpress_not_configured");
    };
    let aliexpress = app_context::<Arc<Aliexpress>>(cx);
    let client = aliexpress
        .app_client(settings)
        .map_err(topcoat::Error::from_anyhow)?;
    let tokens = match client.create_token(&code).await {
        Ok(tokens) => tokens,
        Err(err) => {
            tracing::warn!(tenant = %access.tenant.slug, %err, "AliExpress refused the code");
            return refused("aliexpress_connect_refused");
        }
    };
    let now = crate::auth::now();
    let seal = |plain: &str| aliexpress.seal(plain).map_err(topcoat::Error::from_anyhow);
    let authorized = tenancy(cx)
        .authorize_aliexpress(
            &access.user_id,
            &access.tenant.id,
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
        Ok(()) => Ok(see_other(aliexpress_url(
            cx,
            slug,
            None,
            Some("AliExpress account connected."),
        ))),
        Err(err) if err.is_user_facing() => refused(error_code(&err)),
        Err(err) => Err(err.into()),
    }
}
