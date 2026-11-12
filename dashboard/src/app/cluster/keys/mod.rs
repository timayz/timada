//! `/cluster/keys`: the keys personal data is sealed under, as the nodes copy
//! them between them. `GET` lists the tenants this node keeps keys for, to a
//! node that joined and copies them all
//! ([`KeyVault::copy_from_peers`](crate::key_vault::KeyVault::copy_from_peers)).
//! Only the nodes may: nothing outside the cluster ever reads a key.

pub mod tenant_id;

use std::sync::Arc;

use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{
        StatusCode,
        content::Json,
        header::{AUTHORIZATION, CACHE_CONTROL},
        request::headers,
        response::{IntoResponse, Response},
        route,
    },
};

use crate::key_vault::KeyVault;

/// Whether the request carries the cluster's secret.
pub fn authorized(cx: &Cx, vault: &KeyVault) -> bool {
    vault.authorizes(headers(cx).get(AUTHORIZATION).map(|value| value.as_bytes()))
}

/// The tenants this node keeps keys for. 401 without the cluster's secret.
#[route(GET)]
pub async fn get(cx: &Cx) -> Result<Response> {
    let vault = app_context::<Arc<KeyVault>>(cx);
    if !authorized(cx, vault) {
        return StatusCode::UNAUTHORIZED.into_response(cx);
    }
    let tenants = vault.tenants().await.map_err(topcoat::Error::from_anyhow)?;
    ([(CACHE_CONTROL, "no-store")], Json(tenants)).into_response(cx)
}
