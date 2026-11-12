//! The keys of one tenant, as copied between nodes. `GET` hands out a page
//! of them, to a node that joined and copies them all.

pub mod key_id;

use std::sync::Arc;

use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{
        StatusCode,
        content::Json,
        header::CACHE_CONTROL,
        module_param, path_param, query_params,
        response::{IntoResponse, Response},
        route,
    },
};

use crate::key_vault::KeyVault;
use crate::peers::is_id;

module_param!(pub tenant_id);

#[topcoat::router::query_params(error = bad_request)]
struct PageQuery {
    /// The id of the last key of the page before.
    after: Option<String>,
}

/// Up to [`PAGE_LEN`](crate::key_vault::PAGE_LEN) of the tenant's keys,
/// destroyed ones included, in their ids' order after `after`. 401 without
/// the cluster's secret, 400 for a name that is not an id.
#[route(GET)]
pub async fn get(cx: &Cx) -> Result<Response> {
    let vault = app_context::<Arc<KeyVault>>(cx);
    if !super::authorized(cx, vault) {
        return StatusCode::UNAUTHORIZED.into_response(cx);
    }
    let after = query_params::<PageQuery>(cx)?.after.as_deref();
    let tenant_id = path_param::<TenantId>(cx);
    if !is_id(tenant_id) || after.is_some_and(|after| !is_id(after)) {
        return StatusCode::BAD_REQUEST.into_response(cx);
    }
    let page = vault
        .page(tenant_id, after)
        .await
        .map_err(topcoat::Error::from_anyhow)?;
    ([(CACHE_CONTROL, "no-store")], Json(page)).into_response(cx)
}
