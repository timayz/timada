//! One picture a peer stored and copies here before recording it.

use std::sync::Arc;

use bytes::Bytes;
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{
        StatusCode, header::AUTHORIZATION, module_param, path_param, request::headers, route,
    },
};

use super::TenantId;
use crate::media::Media;

module_param!(pub file);

/// Stores the copy. 401 without the cluster's secret, 422 for a name or
/// bytes that are not a picture the shop takes.
#[route(PUT)]
pub async fn put(cx: &Cx, bytes: Bytes) -> Result<StatusCode> {
    let media = app_context::<Arc<Media>>(cx);
    if !media.authorizes(headers(cx).get(AUTHORIZATION).map(|value| value.as_bytes())) {
        return Ok(StatusCode::UNAUTHORIZED);
    }
    let tenant_id = path_param::<TenantId>(cx);
    let file = path_param::<File>(cx);
    let stored = media
        .accept(tenant_id, file, &bytes)
        .await
        .map_err(topcoat::Error::from_anyhow)?;
    Ok(if stored {
        StatusCode::NO_CONTENT
    } else {
        StatusCode::UNPROCESSABLE_ENTITY
    })
}
