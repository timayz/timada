//! `POST /stripe/webhooks/{tenant_id}`: a tenant's Stripe account telling of
//! its payments ([`crate::payments::webhook`] does the work). Any node may
//! take it.
//!
//! Answers: 204 when handled or ignored; 400 for a bad signature or body;
//! 404 for a tenant without payments; 503 (or a 500) when Stripe should try
//! again later.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{StatusCode, module_param, path_param, request::headers, route},
};

use crate::app::tenants::tenancy;
use crate::cluster::Executor;
use crate::payments::Payments;
use crate::payments::webhook::{Outcome, receive as handle};

module_param!(pub tenant_id);

#[route(POST)]
pub async fn receive(cx: &Cx, body: Bytes) -> Result<StatusCode> {
    let payments = app_context::<Arc<Payments>>(cx);
    let tenant = tenancy(cx)
        .load_tenant(path_param::<TenantId>(cx))
        .await
        .map_err(topcoat::Error::from_anyhow)?;
    let Some(tenant) = tenant.filter(|tenant| !tenant.is_suspended()) else {
        return Ok(StatusCode::NOT_FOUND);
    };
    let Some(account) = payments
        .account_of(&tenant)
        .map_err(topcoat::Error::from_anyhow)?
    else {
        return Ok(StatusCode::NOT_FOUND);
    };
    let signature = headers(cx)
        .get("stripe-signature")
        .and_then(|value| value.to_str().ok());
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs() as i64);
    let outcome = handle(
        payments,
        &account,
        &tenant.id,
        app_context::<Executor>(cx).clone(),
        &body,
        signature,
        now,
    )
    .await
    .map_err(topcoat::Error::from_anyhow)?;
    Ok(match outcome {
        Outcome::Placed { .. }
        | Outcome::Refunded { .. }
        | Outcome::Returned { .. }
        | Outcome::Ignored => StatusCode::NO_CONTENT,
        Outcome::Rejected => {
            tracing::warn!(tenant = %tenant.slug, "a Stripe webhook that does not check out");
            StatusCode::BAD_REQUEST
        }
        Outcome::Retry => StatusCode::SERVICE_UNAVAILABLE,
    })
}
