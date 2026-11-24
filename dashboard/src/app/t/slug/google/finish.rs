use timada_google::Connect;
use timada_tenancy::PERMISSION_PAGES_MANAGE;
use topcoat::{
    Result,
    context::{Cx, context},
    router::{
        error::{SeeOther, see_other},
        query_params, route,
    },
};

use super::{error_code, google, google_url};
use crate::app::google::callback_url;
use crate::app::t::slug::require_tenant;
use crate::auth::cookies::{google_flow, is_fresh};
use crate::google::{Google, GoogleFault};

#[topcoat::router::query_params(error = bad_request)]
struct FinishQuery {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
}

/// Where the callback hands a member on with what Google answered:
/// exchanges the code for the account's grant and keeps it, sealed, on the
/// shop's connection. The grant replaces the one held: it covers what that
/// one did.
#[route(GET)]
pub async fn finish(cx: &Cx) -> Result<SeeOther> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_PAGES_MANAGE)
        .await?;
    let query = query_params::<FinishQuery>(cx)?;
    let slug = access.tenant.slug.as_str();
    let tenant_id = access.tenant_id();
    let refused = |code: &str| Ok(see_other(google_url(cx, slug, Some(code), None)));
    // One shot: whatever happens next, the flow cannot be replayed.
    let store = google_flow(cx);
    let flow = match store.parse() {
        Ok(Some(store)) => {
            let flow = store.get();
            store.remove();
            flow
        }
        Ok(None) | Err(_) => return refused("google_connect_expired"),
    };
    if flow.tenant_id != tenant_id
        || flow.user_id != access.user_id
        || !is_fresh(flow.issued_at, crate::auth::now())
        || query.state.as_deref() != Some(flow.state.as_str())
    {
        return refused("google_connect_expired");
    }
    let Some(code) = query
        .code
        .as_deref()
        .filter(|code| !code.is_empty() && query.error.is_none())
    else {
        return refused("google_connect_refused");
    };
    let service = context::<Google>(cx);
    let grant = match service.exchange(code, &callback_url(cx)).await {
        Ok(grant) => grant,
        Err(GoogleFault::Off) => return refused("google_disabled"),
        Err(GoogleFault::Unavailable(reason)) => {
            tracing::warn!(tenant = %tenant_id, %reason, "Google did not exchange a code");
            return refused("google_unavailable");
        }
        Err(fault) => {
            tracing::warn!(tenant = %tenant_id, %fault, "Google refused a code");
            return refused("google_connect_refused");
        }
    };
    if grant.accesses.is_empty() {
        return refused("google_nothing_granted");
    }
    let Some(refresh_token) = &grant.refresh_token else {
        tracing::warn!(tenant = %tenant_id, "Google gave a grant that cannot be renewed");
        return refused("google_connect_refused");
    };
    let account = match service.account(&grant.access_token).await {
        Ok(account) => account,
        Err(fault) => {
            tracing::warn!(tenant = %tenant_id, %fault, "Google did not name the account of a grant");
            return refused("google_unavailable");
        }
    };
    let sealed = service
        .seal(tenant_id, refresh_token)
        .map_err(topcoat::Error::from_anyhow)?;
    let connected = google(cx)
        .connect(
            &access.user_id,
            tenant_id,
            Connect {
                refresh_token_sealed: sealed.clone(),
                account,
                accesses: grant.accesses.clone(),
            },
        )
        .await;
    match connected {
        Ok(()) => {
            service.keep(tenant_id, &sealed, &grant);
            Ok(see_other(google_url(
                cx,
                slug,
                None,
                Some("Google account connected."),
            )))
        }
        Err(err) if err.is_user_facing() => refused(error_code(&err)),
        Err(err) => Err(topcoat::Error::from_anyhow(err.into())),
    }
}
