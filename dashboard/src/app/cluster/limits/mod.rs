//! `POST /cluster/limits`: how often this node let the asked keys through
//! within a window, so the asking node holds a client to one limit for the
//! whole cluster (see [`crate::rate_limit`]). Only the nodes may ask.

use std::sync::Arc;

use bytes::Bytes;
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{
        StatusCode,
        content::Json,
        header::AUTHORIZATION,
        request::headers,
        response::{IntoResponse, Response},
        route,
    },
};

use crate::rate_limit::RateLimiter;

/// This node's own passes of each key asked. 401 without the cluster's
/// secret, 400 for a body that is not a question.
#[route(POST)]
pub async fn post(cx: &Cx, bytes: Bytes) -> Result<Response> {
    let limiter = app_context::<Arc<RateLimiter>>(cx);
    if !limiter.authorizes(headers(cx).get(AUTHORIZATION).map(|value| value.as_bytes())) {
        return StatusCode::UNAUTHORIZED.into_response(cx);
    }
    match limiter.answer(&bytes) {
        Ok(counted) => (StatusCode::OK, Json(counted)).into_response(cx),
        Err(err) => {
            tracing::debug!(%err, "a peer's question about the limits is not one");
            StatusCode::BAD_REQUEST.into_response(cx)
        }
    }
}
