//! One shopper key: a peer stores it here before recording the account, and
//! a peer lacking it asks for it here. Only the nodes may: nothing outside
//! the cluster ever reads a key.

use std::sync::Arc;

use bytes::Bytes;
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{
        StatusCode,
        header::{AUTHORIZATION, CACHE_CONTROL, CONTENT_TYPE},
        module_param, path_param,
        request::headers,
        response::{IntoResponse, Response},
        route,
    },
};

use super::TenantId;
use crate::shopper_keys::ShopperKeyVault;

module_param!(pub key_id);

fn authorized(cx: &Cx, vault: &ShopperKeyVault) -> bool {
    vault.authorizes(headers(cx).get(AUTHORIZATION).map(|value| value.as_bytes()))
}

/// Stores the copy. 401 without the cluster's secret, 422 for a name or
/// bytes that are not a key, or a key this node destroyed.
#[route(PUT)]
pub async fn put(cx: &Cx, bytes: Bytes) -> Result<StatusCode> {
    let vault = app_context::<Arc<ShopperKeyVault>>(cx);
    if !authorized(cx, vault) {
        return Ok(StatusCode::UNAUTHORIZED);
    }
    let stored = vault
        .accept(path_param::<TenantId>(cx), path_param::<KeyId>(cx), &bytes)
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
    let vault = app_context::<Arc<ShopperKeyVault>>(cx);
    if !authorized(cx, vault) {
        return StatusCode::UNAUTHORIZED.into_response(cx);
    }
    let sealed = vault
        .sealed(path_param::<TenantId>(cx), path_param::<KeyId>(cx))
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
