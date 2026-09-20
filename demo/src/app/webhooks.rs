//! `POST /webhooks/stripe`: what Stripe reports — payments received, refunds
//! gone through or not. The signature is checked against the raw body before
//! anything is read; the event then goes to the payment context, which is
//! safe to tell the same thing twice (Stripe delivers at least once).

use timada_payment::{WebhookError, apply_provider_event};
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{
        StatusCode,
        request::{Bytes, headers},
        route,
    },
};

use crate::Store;

/// 2xx acknowledges; anything else makes Stripe try again later — right for
/// a failure on our side, pointless for a request that is not Stripe's.
#[route(POST "/webhooks/stripe")]
pub async fn stripe(cx: &Cx, body: Bytes) -> Result<StatusCode> {
    let store = app_context::<Store>(cx);
    let Some(stripe) = &store.stripe else {
        return Ok(StatusCode::NOT_FOUND);
    };
    let signature = headers(cx)
        .get("stripe-signature")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    let now = timada_core::time::now_unix_secs()?;
    let event = match stripe.parse_webhook(signature, &body, now) {
        Ok(Some(event)) => event,
        // Signed by Stripe, of no use to the shop: acknowledged.
        Ok(None) => return Ok(StatusCode::OK),
        Err(err @ WebhookError::Payload(_)) => {
            tracing::error!(%err, "unreadable Stripe event");
            return Ok(StatusCode::BAD_REQUEST);
        }
        Err(err) => {
            tracing::warn!(%err, "Stripe webhook refused");
            return Ok(StatusCode::BAD_REQUEST);
        }
    };

    let applied =
        apply_provider_event(&store.executor, &store.db, stripe.as_ref(), event.clone()).await;
    match applied {
        Ok(applied) => {
            tracing::info!(?event, ?applied, "Stripe event");
            Ok(StatusCode::OK)
        }
        Err(err) => {
            tracing::error!(%err, ?event, "Stripe event not applied; Stripe will send it again");
            Ok(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}
