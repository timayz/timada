//! `POST /store/newsletter` `email=…&language=fr`: a storefront hands in an
//! address that asks for the shop's news, from its sign-up form. The shop
//! mails it a link to confirm (`{storefront}/newsletter/confirm/{token}`,
//! which the storefront hands back through [`confirm`]); the address counts
//! only once it is followed. `language` is a tag or a whole
//! `Accept-Language` value (optional), the language the link is mailed in;
//! `time_zone` the IANA zone the visitor's browser gives (optional, anything
//! else ignored), the hours a scheduled campaign keeps for them.
//!
//! Answers 202 `{}` whether the address was mailed, is on the list already
//! or was mailed less than an hour ago, so the answer says nothing of who is
//! subscribed; 422 `{"error": code}` for a refusal to show the visitor (the
//! codes of `timada_newsletter::SHOPPER_CODES`); 401 for a missing, unknown
//! or revoked key, 403 for a key that may not sell, 503 for a suspended
//! tenant, a shop not taking writes for the moment, or one that has not said
//! where its storefront lives (`{"error": "newsletter_unavailable"}`).
//!
//! Each key, and each visitor at a shop, may hand in only so many sign-ups
//! within `[store]`'s window: past it, 429 `{"error": "too_many_requests"}`
//! and nothing is recorded or mailed. The visitor is the address the
//! storefront reports in `X-Shopper-IP`, else the caller's.

pub mod confirm;
pub mod unsubscribe;

use std::sync::Arc;

use serde::Deserialize;
use timada_newsletter::{
    NEWSLETTER_TOO_MANY_REQUESTS, NEWSLETTER_UNAVAILABLE, NewsletterError, RequestSubscription,
    SOURCE_FORM,
};
use timada_orders::STOREFRONT_ACTOR;
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

use super::shoppers::{Answer, refused, status};
use crate::auth::now;
use crate::config::Config;
use crate::newsletter::mail_confirmation;
use crate::rate_limit::{RateLimiter, allow_newsletter_signup};

/// The header a storefront reports its visitor's address in.
const SHOPPER_IP: &str = "x-shopper-ip";

#[derive(Deserialize)]
pub struct SignupForm {
    #[serde(default)]
    email: String,
    #[serde(default)]
    language: Option<String>,
    #[serde(default)]
    time_zone: Option<String>,
}

#[route(POST)]
pub async fn subscribe(cx: &Cx, Form(form): Form<SignupForm>) -> Result<Answer> {
    let access = match super::storefront(cx).await? {
        Ok(access) => access,
        Err(code) => return status(code),
    };
    let draft = RequestSubscription {
        email: form.email,
        language: form.language,
        source: SOURCE_FORM.to_owned(),
        time_zone: form.time_zone,
    };
    // Refused before anything is counted or opened when it can be.
    if let Err(err) = draft.validate() {
        return answer_error(err);
    }
    let allowed = allow_newsletter_signup(
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
            "newsletter sign-up refused: too many handed in"
        );
        return refused(StatusCode::TOO_MANY_REQUESTS, NEWSLETTER_TOO_MANY_REQUESTS);
    }
    let store = super::store(cx, &access);
    let Some(storefront) = timada_orders::Command::new(store.executor().clone(), store.tenant_id())
        .storefront_address()
        .await
        .map_err(topcoat::Error::from_anyhow)?
    else {
        return refused(StatusCode::SERVICE_UNAVAILABLE, NEWSLETTER_UNAVAILABLE);
    };
    let keys = super::shoppers::keys(cx, &access.tenant.id);
    let outcome = match timada_newsletter::Command::new(store.executor().clone(), store.tenant_id())
        .request_subscription(&*keys, STOREFRONT_ACTOR, draft, now())
        .await
    {
        Ok(outcome) => outcome,
        Err(err) => return answer_error(err),
    };
    mail_confirmation(cx, &access.tenant, &storefront, &outcome).await?;
    status(StatusCode::ACCEPTED)
}

/// 422 with the refusal's code, or the error itself for an infrastructure
/// failure.
pub fn answer_error(err: NewsletterError) -> Result<Answer> {
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
    /// storefront words as "try again later"; a refusal of the request
    /// itself stays 422, and a real failure is not an answer at all.
    #[test]
    fn a_paused_store_answers_service_unavailable() {
        let paused = NewsletterError::Write(evento::WriteError::Unavailable(anyhow::anyhow!(
            "backlog at cap"
        )));
        let (status, body) = answer_error(paused).expect("an answer");
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(body.0["error"], timada_core::WRITES_PAUSED);

        let (status, body) = answer_error(NewsletterError::InvalidLink).expect("an answer");
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(body.0["error"], timada_newsletter::NEWSLETTER_INVALID_LINK);

        let broken =
            NewsletterError::Write(evento::WriteError::Unknown(anyhow::anyhow!("disk gone")));
        assert!(answer_error(broken).is_err());
    }
}
