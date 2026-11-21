//! One rendition of a picture: a peer stores it and copies it here before
//! recording it, and a peer that lacks it fetches it from here.

use std::sync::Arc;

use bytes::Bytes;
use timada_core::RENDITION_MIME;
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

use super::super::tenant_id;
use super::rendition;
use crate::media::Media;

module_param!(pub file);

/// Stores the copy. 422 for a name or bytes that are not a rendition of a
/// picture.
#[route(PUT)]
pub async fn put(cx: &Cx, bytes: Bytes) -> Result<StatusCode> {
    let media = context::<Arc<Media>>(cx);
    let tenant_id = tenant_id(cx);
    let rendition = rendition(cx);
    let file = file(cx);
    let stored = media
        .accept(tenant_id, rendition, file, &bytes)
        .await
        .map_err(topcoat::Error::from_anyhow)?;
    Ok(if stored {
        StatusCode::NO_CONTENT
    } else {
        StatusCode::UNPROCESSABLE_ENTITY
    })
}

/// The file, for a peer that lacks it. 404 when this node does not hold
/// it.
#[route(GET)]
pub async fn get(cx: &Cx) -> Result<Response> {
    let media = context::<Arc<Media>>(cx);
    let copy = media
        .copy_of(tenant_id(cx), rendition(cx), file(cx))
        .await
        .map_err(topcoat::Error::from_anyhow)?;
    match copy {
        None => StatusCode::NOT_FOUND.into_response(cx),
        Some(bytes) => (
            [(CONTENT_TYPE, RENDITION_MIME), (CACHE_CONTROL, "no-store")],
            bytes,
        )
            .into_response(cx),
    }
}
