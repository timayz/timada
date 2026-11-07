//! `POST /aliexpress/webhook`: AliExpress telling of an order's new status
//! (its message push, registered once on the platform's app;
//! [`crate::aliexpress::webhook`] does the work). Any node may take it.
//!
//! Answers: 200 once the push is recorded, or for another topic, as
//! AliExpress wants (anything else is retried twelve times, so a push the
//! store did not take is an error and comes again); 400 for a body that is
//! not a push; 401 for a signature that does not check out; 404 when the
//! platform has no app.

use std::sync::Arc;

use bytes::Bytes;
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{StatusCode, request::headers, route},
};

use crate::aliexpress::Aliexpress;
use crate::aliexpress::webhook::{Outcome, receive as handle};
use crate::cluster::Executor;

#[route(POST)]
pub async fn receive(cx: &Cx, body: Bytes) -> Result<StatusCode> {
    let aliexpress = app_context::<Arc<Aliexpress>>(cx);
    let authorization = headers(cx)
        .get("authorization")
        .and_then(|value| value.to_str().ok());
    let outcome = handle(
        aliexpress,
        app_context::<Executor>(cx),
        &body,
        authorization,
    )
    .await
    .map_err(topcoat::Error::from_anyhow)?;
    Ok(match outcome {
        Outcome::Accepted { .. } | Outcome::Ignored => StatusCode::OK,
        Outcome::Malformed => StatusCode::BAD_REQUEST,
        Outcome::Rejected => {
            tracing::warn!("an AliExpress push that does not check out");
            StatusCode::UNAUTHORIZED
        }
        Outcome::Disabled => StatusCode::NOT_FOUND,
    })
}
