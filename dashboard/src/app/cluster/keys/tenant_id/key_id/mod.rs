//! One key personal data is sealed under: a peer stores it here before
//! recording what it seals, and a peer lacking it asks for it here. Only the
//! nodes may: nothing outside the cluster ever reads a key.

use std::sync::Arc;

use bytes::Bytes;
use topcoat::{
    Result,
    context::{Cx, context},
    router::{
        StatusCode,
        header::{CACHE_CONTROL, CONTENT_TYPE},
        module_param,
        response::{IntoResponse, Response},
        route,
    },
};

use super::tenant_id;
use crate::app::cluster::keys::authorized;
use crate::key_vault::KeyVault;

module_param!(pub key_id);

/// Stores the copy. 401 without the cluster's secret, 422 for a name or
/// bytes that are not a key, or a key this node destroyed.
#[route(PUT)]
pub async fn put(cx: &Cx, bytes: Bytes) -> Result<StatusCode> {
    let vault = context::<Arc<KeyVault>>(cx);
    if !authorized(cx, vault) {
        return Ok(StatusCode::UNAUTHORIZED);
    }
    let stored = vault
        .accept(tenant_id(cx), key_id(cx), &bytes)
        .await
        .map_err(topcoat::Error::from_anyhow)?;
    Ok(if stored {
        StatusCode::NO_CONTENT
    } else {
        StatusCode::UNPROCESSABLE_ENTITY
    })
}

/// The sealed key. 401 without the cluster's secret, 404 when this node
/// does not hold it.
#[route(GET)]
pub async fn get(cx: &Cx) -> Result<Response> {
    let vault = context::<Arc<KeyVault>>(cx);
    if !authorized(cx, vault) {
        return StatusCode::UNAUTHORIZED.into_response(cx);
    }
    let sealed = vault
        .sealed(tenant_id(cx), key_id(cx))
        .await
        .map_err(topcoat::Error::from_anyhow)?;
    match sealed {
        None => StatusCode::NOT_FOUND.into_response(cx),
        Some(sealed) => (
            [
                (CONTENT_TYPE, "application/octet-stream"),
                (CACHE_CONTROL, "no-store"),
            ],
            sealed,
        )
            .into_response(cx),
    }
}
