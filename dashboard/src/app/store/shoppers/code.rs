//! `POST /store/shoppers/code` `email=…&language=fr`: issues a sign-in code
//! for the address, which the shop mails right after
//! ([`crate::code_mails`]), in the shopper's language when the shop has that
//! wording (`language` is a tag or a whole `Accept-Language` value;
//! optional). Any address gets one: the first sign-in creates the account.
//! 202 `{}` once the code is recorded; 429 `{"error": "code_recently_sent"}`
//! within a minute of the last one, unless that one could not be delivered.
//!
//! Each key, and each shopper at a shop, may have only so many codes mailed
//! within `[store]`'s window: past it, 429 `{"error": "too_many_requests"}`
//! and nothing is mailed. The shopper is the address the storefront reports
//! in `X-Shopper-IP`, else the caller's.

use std::sync::Arc;

use serde::Deserialize;
use timada_shoppers::SHOPPER_TOO_MANY_REQUESTS;
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{
        StatusCode,
        content::Form,
        request::{client_ip, headers},
        route,
    },
};

use super::{Answer, answer_error, refused, status};
use crate::auth::now;
use crate::config::Config;
use crate::rate_limit::{RateLimiter, allow_shopper_code};

/// The header a storefront reports its shopper's address in.
const SHOPPER_IP: &str = "x-shopper-ip";

#[derive(Deserialize)]
pub struct CodeForm {
    email: String,
    #[serde(default)]
    language: Option<String>,
}

#[route(POST)]
pub async fn code(cx: &Cx, Form(form): Form<CodeForm>) -> Result<Answer> {
    let access = match super::super::storefront(cx).await? {
        Ok(access) => access,
        Err(code) => return status(code),
    };
    let allowed = allow_shopper_code(
        app_context::<Arc<RateLimiter>>(cx),
        &app_context::<Config>(cx).store,
        &access.key.id,
        &access.tenant.id,
        headers(cx)
            .get(SHOPPER_IP)
            .and_then(|value| value.to_str().ok()),
        client_ip(cx),
    )
    .await;
    if !allowed {
        tracing::info!(
            tenant = %access.tenant.slug,
            key = %access.key.id,
            "sign-in code refused: too many asked for"
        );
        return refused(StatusCode::TOO_MANY_REQUESTS, SHOPPER_TOO_MANY_REQUESTS);
    }
    let codes = match super::codes(cx) {
        Ok(codes) => codes,
        Err(answer) => return answer,
    };
    let store = super::super::store(cx, &access);
    let keys = super::keys(cx, &access.tenant.id);
    // Recorded here, mailed by the subscription that follows the stream.
    // Read through the cluster: whether the address's account was deleted
    // decides the code's key, and a node behind may not know it yet.
    if let Err(err) = timada_shoppers::Command::new(store.executor().clone(), store.tenant_id())
        .issue_code(
            &*keys,
            &*codes,
            &form.email,
            form.language.as_deref(),
            now(),
        )
        .await
    {
        return answer_error(err);
    }
    status(StatusCode::ACCEPTED)
}
