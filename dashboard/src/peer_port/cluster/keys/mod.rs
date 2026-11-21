//! `/cluster/keys`: the keys personal data is sealed under, as the nodes copy
//! them between them. `GET` lists the tenants this node keeps keys for, to a
//! node that joined and copies them all
//! ([`KeyVault::copy_from_peers`](crate::key_vault::KeyVault::copy_from_peers)).
//! Only the nodes reach it: nothing outside the cluster ever reads a key.

pub mod tenant_id;

use std::sync::Arc;

use topcoat::{
    Result,
    context::{Cx, context},
    router::{
        content::Json,
        header::CACHE_CONTROL,
        response::{IntoResponse, Response},
        route,
    },
};

use crate::key_vault::KeyVault;

/// The tenants this node keeps keys for.
#[route(GET)]
pub async fn get(cx: &Cx) -> Result<Response> {
    let vault = context::<Arc<KeyVault>>(cx);
    let tenants = vault.tenants().await.map_err(topcoat::Error::from_anyhow)?;
    ([(CACHE_CONTROL, "no-store")], Json(tenants)).into_response(cx)
}
