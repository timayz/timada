//! `POST /store/contact` `first_name=…&last_name=…&email=…&body=…&language=fr`:
//! a storefront hands in what a visitor wrote in the shop's contact form.
//! The key (`Authorization: Bearer tk_…`) names the tenant and must allow
//! `store:checkout`; `language` is a tag or a whole `Accept-Language` value
//! (optional), so the merchant knows which language to answer in.
//!
//! Answers: 201 `{"id": …}`; 422 `{"error": code}` for a refusal to show
//! the visitor (the codes of `timada_support::SHOPPER_CODES`); 401 for a
//! missing, unknown or revoked key, 403 for a key that may not sell, 503 for
//! a suspended tenant or a shop not taking writes for the moment.
//!
//! Each key, and each visitor at a shop, may hand in only so many messages
//! within `[store]`'s window: past it, 429 `{"error": "too_many_requests"}`
//! and nothing is recorded. The visitor is the address the storefront
//! reports in `X-Shopper-IP`, else the caller's.

use std::sync::Arc;

use serde::Deserialize;
use serde_json::json;
use timada_orders::STOREFRONT_ACTOR;
use timada_support::{ReceiveMessage, SUPPORT_TOO_MANY_REQUESTS, SupportError};
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

use super::shoppers::{Answer, answer, refused, status};
use crate::config::Config;
use crate::rate_limit::{RateLimiter, allow_contact_message};

/// The header a storefront reports its visitor's address in.
const SHOPPER_IP: &str = "x-shopper-ip";

#[derive(Deserialize)]
pub struct ContactForm {
    #[serde(default)]
    first_name: String,
    #[serde(default)]
    last_name: String,
    #[serde(default)]
    email: String,
    #[serde(default)]
    body: String,
    #[serde(default)]
    language: Option<String>,
}

#[route(POST)]
pub async fn contact(cx: &Cx, Form(form): Form<ContactForm>) -> Result<Answer> {
    let access = match super::storefront(cx).await? {
        Ok(access) => access,
        Err(code) => return status(code),
    };
    let allowed = allow_contact_message(
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
            "contact message refused: too many sent"
        );
        return refused(StatusCode::TOO_MANY_REQUESTS, SUPPORT_TOO_MANY_REQUESTS);
    }
    let draft = ReceiveMessage {
        first_name: form.first_name,
        last_name: form.last_name,
        email: form.email,
        body: form.body,
        language: form.language,
    };
    // Refused before the store is opened when it can be.
    if let Err(err) = draft.validate() {
        return answer_error(err);
    }
    let store = super::store(cx, &access);
    let keys = super::shoppers::keys(cx, &access.tenant.id);
    match timada_support::Command::new(store.executor().clone(), store.tenant_id())
        .receive_message(&*keys, STOREFRONT_ACTOR, draft)
        .await
    {
        Ok(message) => answer(StatusCode::CREATED, json!({ "id": message.id })),
        Err(err) => answer_error(err),
    }
}

/// 422 with the refusal's code, or the error itself for an infrastructure
/// failure.
fn answer_error(err: SupportError) -> Result<Answer> {
    match err.code() {
        // The shop is not taking writes for the moment: not the visitor's
        // doing, so a 503 the storefront can word as "try again later".
        Some(code) if code == timada_core::WRITES_PAUSED => {
            refused(StatusCode::SERVICE_UNAVAILABLE, code)
        }
        Some(code) if err.is_user_facing() => refused(StatusCode::UNPROCESSABLE_ENTITY, code),
        _ => Err(topcoat::Error::from_anyhow(err.into())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A shop not taking writes for the moment answers 503 with the code a
    /// storefront words as "try again later"; a refusal of the message itself
    /// stays 422, and a real failure is not an answer at all.
    #[test]
    fn a_paused_store_answers_service_unavailable() {
        let paused = SupportError::Write(evento::WriteError::Unavailable(anyhow::anyhow!(
            "backlog at cap"
        )));
        let (status, body) = answer_error(paused).expect("an answer");
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(body.0["error"], timada_core::WRITES_PAUSED);

        let (status, body) = answer_error(SupportError::InvalidBody).expect("an answer");
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(body.0["error"], timada_support::MESSAGE_INVALID_BODY);

        let broken = SupportError::Write(evento::WriteError::Unknown(anyhow::anyhow!("disk gone")));
        assert!(answer_error(broken).is_err());
    }
}
