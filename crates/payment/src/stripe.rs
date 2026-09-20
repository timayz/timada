//! Stripe as the shop's [`PaymentProvider`] (feature `stripe`): a
//! PaymentIntent per payment, paid on Stripe's embedded card form
//! ([`PaymentStart::ClientSecret`]), refunds, and the verification of Stripe's
//! webhooks. A thin client over the three REST calls it needs — no SDK.
//!
//! The host owns the webhook route: it hands the raw body and the
//! `Stripe-Signature` header to [`StripeProvider::parse_webhook`] and what
//! comes back to [`crate::apply_provider_event`].

use std::time::Duration;

use hmac::{Hmac, Mac};
use serde::Deserialize;
use sha2::Sha256;
use timada_core::Money;

use crate::{
    provider::{
        CancelOutcome, PaymentProvider, PaymentStart, ProviderError, ProviderFuture,
        ProviderRefund, RefundOutcome, ReturnUrls, StartedPayment,
    },
    provider_event::ProviderEvent,
    query::PaymentView,
    value_object::PaymentMethod,
};

/// How far a webhook's timestamp may be from now, either way.
pub const WEBHOOK_TOLERANCE_SECS: u64 = 300;

/// The keys of a Stripe account. `Debug` never shows the secrets.
#[derive(Clone)]
pub struct StripeConfig {
    /// `sk_…`: server side only.
    pub secret_key: String,
    /// `pk_…`: handed to the browser with the client secret.
    pub publishable_key: String,
    /// `whsec_…`: the signing secret of the webhook endpoint.
    pub webhook_secret: String,
    /// `https://api.stripe.com` unless a test points elsewhere.
    pub api_base: String,
}

impl StripeConfig {
    pub fn new(
        secret_key: impl Into<String>,
        publishable_key: impl Into<String>,
        webhook_secret: impl Into<String>,
    ) -> Self {
        Self {
            secret_key: secret_key.into(),
            publishable_key: publishable_key.into(),
            webhook_secret: webhook_secret.into(),
            api_base: "https://api.stripe.com".to_owned(),
        }
    }
}

impl std::fmt::Debug for StripeConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StripeConfig")
            .field("publishable_key", &self.publishable_key)
            .field("api_base", &self.api_base)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum WebhookError {
    #[error("malformed Stripe-Signature header")]
    MalformedSignature,
    #[error("signature does not match")]
    BadSignature,
    #[error("timestamp outside the tolerance")]
    Stale,
    #[error("payload is not a Stripe event: {0}")]
    Payload(String),
}

#[derive(Debug, Clone)]
pub struct StripeProvider {
    http: reqwest::Client,
    config: StripeConfig,
}

#[derive(Debug, Deserialize)]
struct Intent {
    id: String,
    status: String,
    client_secret: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Refund {
    id: String,
    status: Option<String>,
    failure_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ErrorBody {
    error: ErrorDetail,
}

#[derive(Debug, Deserialize)]
struct ErrorDetail {
    message: Option<String>,
    code: Option<String>,
}

/// A Stripe call that did not answer 2xx.
struct ApiError {
    status: reqwest::StatusCode,
    code: Option<String>,
    message: String,
}

impl ApiError {
    /// Rate limits, lock timeouts, idempotent requests still in flight and
    /// Stripe's own failures pass; the rest is Stripe saying no.
    fn into_provider_error(self) -> ProviderError {
        let retryable = self.status.is_server_error()
            || self.status == reqwest::StatusCode::TOO_MANY_REQUESTS
            || self.status == reqwest::StatusCode::CONFLICT
            || self.status == reqwest::StatusCode::UNAUTHORIZED;
        if retryable {
            ProviderError::Unavailable(self.message)
        } else {
            ProviderError::Refused(self.message)
        }
    }
}

enum CallError {
    Api(ApiError),
    Transport(String),
}

impl From<CallError> for ProviderError {
    fn from(err: CallError) -> Self {
        match err {
            CallError::Api(api) => api.into_provider_error(),
            CallError::Transport(message) => ProviderError::Unavailable(message),
        }
    }
}

impl StripeProvider {
    pub fn new(config: StripeConfig) -> Result<Self, ProviderError> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|err| ProviderError::Unavailable(err.to_string()))?;
        Ok(Self { http, config })
    }

    async fn call<T: for<'de> Deserialize<'de>>(
        &self,
        request: reqwest::RequestBuilder,
        idempotency_key: Option<&str>,
    ) -> Result<T, CallError> {
        let mut request = request.basic_auth(&self.config.secret_key, None::<&str>);
        if let Some(key) = idempotency_key {
            request = request.header("Idempotency-Key", key);
        }
        let response = request
            .send()
            .await
            .map_err(|err| CallError::Transport(err.to_string()))?;
        let status = response.status();
        let body = response
            .bytes()
            .await
            .map_err(|err| CallError::Transport(err.to_string()))?;
        if status.is_success() {
            return serde_json::from_slice(&body)
                .map_err(|err| CallError::Transport(format!("unreadable answer: {err}")));
        }
        let detail = serde_json::from_slice::<ErrorBody>(&body)
            .ok()
            .map(|b| b.error);
        let code = detail.as_ref().and_then(|d| d.code.clone());
        let message = detail
            .and_then(|d| d.message)
            .unwrap_or_else(|| format!("HTTP {status}"));
        Err(CallError::Api(ApiError {
            status,
            code,
            message,
        }))
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.config.api_base.trim_end_matches('/'))
    }

    async fn retrieve_intent(&self, id: &str) -> Result<Intent, CallError> {
        self.call(
            self.http
                .get(self.url(&format!("/v1/payment_intents/{id}"))),
            None,
        )
        .await
    }

    async fn create_intent(&self, payment: &PaymentView, key: &str) -> Result<Intent, CallError> {
        let form = [
            ("amount", payment.amount.minor.to_string()),
            ("currency", payment.amount.currency.to_lowercase()),
            ("payment_method_types[]", "card".to_owned()),
            ("metadata[payment_id]", payment.id.clone()),
            ("metadata[order_id]", payment.order_id.clone()),
        ];
        self.call(
            self.http.post(self.url("/v1/payment_intents")).form(&form),
            Some(key),
        )
        .await
    }

    fn started(&self, intent: Intent) -> Result<StartedPayment, ProviderError> {
        let client_secret = intent.client_secret.ok_or_else(|| {
            ProviderError::Unavailable("payment intent came without a client secret".to_owned())
        })?;
        Ok(StartedPayment {
            session_reference: Some(intent.id),
            start: PaymentStart::ClientSecret {
                client_secret,
                publishable_key: self.config.publishable_key.clone(),
            },
        })
    }

    /// Checks a webhook's `Stripe-Signature` against its raw `payload` and
    /// reads the event. `Ok(None)` is an event this shop has no use for — to
    /// acknowledge all the same, or Stripe keeps sending it.
    pub fn parse_webhook(
        &self,
        signature: &str,
        payload: &[u8],
        now_unix_secs: u64,
    ) -> Result<Option<ProviderEvent>, WebhookError> {
        verify_signature(
            &self.config.webhook_secret,
            signature,
            payload,
            now_unix_secs,
        )?;
        read_event(payload)
    }
}

/// The `Stripe-Signature` header Stripe would send for `payload` at
/// `timestamp` — for a host's own webhook tests.
pub fn sign_webhook(secret: &str, timestamp: u64, payload: &[u8]) -> String {
    let signature = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).map_or_else(
        |_| String::new(),
        |mut mac| {
            mac.update(format!("{timestamp}.").as_bytes());
            mac.update(payload);
            hex::encode(mac.finalize().into_bytes())
        },
    );
    format!("t={timestamp},v1={signature}")
}

/// `Stripe-Signature: t=<unix>,v1=<hex hmac-sha256 of "{t}.{payload}">[,v1=…]`.
fn verify_signature(
    secret: &str,
    header: &str,
    payload: &[u8],
    now: u64,
) -> Result<(), WebhookError> {
    let mut timestamp = None;
    let mut signatures = Vec::new();
    for part in header.split(',') {
        match part.trim().split_once('=') {
            Some(("t", value)) => timestamp = value.parse::<u64>().ok(),
            Some(("v1", value)) => signatures.push(value),
            _ => {}
        }
    }
    let timestamp = timestamp.ok_or(WebhookError::MalformedSignature)?;
    if signatures.is_empty() {
        return Err(WebhookError::MalformedSignature);
    }

    let matches = signatures.iter().any(|signature| {
        let Ok(signature) = hex::decode(signature) else {
            return false;
        };
        let Ok(mut mac) = Hmac::<Sha256>::new_from_slice(secret.as_bytes()) else {
            return false;
        };
        mac.update(timestamp.to_string().as_bytes());
        mac.update(b".");
        mac.update(payload);
        // Constant time.
        mac.verify_slice(&signature).is_ok()
    });
    if !matches {
        return Err(WebhookError::BadSignature);
    }
    // Checked after the signature, so only Stripe learns its clock is off.
    if now.abs_diff(timestamp) > WEBHOOK_TOLERANCE_SECS {
        return Err(WebhookError::Stale);
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
struct WebhookEvent {
    #[serde(rename = "type")]
    kind: String,
    data: WebhookData,
}

#[derive(Debug, Deserialize)]
struct WebhookData {
    object: serde_json::Value,
}

#[derive(Debug, Deserialize)]
struct PaidIntent {
    id: String,
    amount_received: i64,
    currency: String,
    #[serde(default)]
    metadata: std::collections::HashMap<String, String>,
}

fn read_event(payload: &[u8]) -> Result<Option<ProviderEvent>, WebhookError> {
    let event: WebhookEvent =
        serde_json::from_slice(payload).map_err(|err| WebhookError::Payload(err.to_string()))?;
    let object = |err: serde_json::Error| WebhookError::Payload(err.to_string());
    match event.kind.as_str() {
        "payment_intent.succeeded" => {
            let intent: PaidIntent = serde_json::from_value(event.data.object).map_err(object)?;
            // An intent made outside the shop on the same account.
            let Some(payment_id) = intent.metadata.get("payment_id") else {
                return Ok(None);
            };
            Ok(Some(ProviderEvent::Paid {
                payment_id: payment_id.clone(),
                reference: intent.id,
                amount: Money::new(intent.amount_received, intent.currency.to_uppercase()),
            }))
        }
        // `charge.refund.updated` is what older API versions call it.
        "refund.updated" | "refund.failed" | "charge.refund.updated" => {
            let refund: Refund = serde_json::from_value(event.data.object).map_err(object)?;
            Ok(match refund.status.as_deref() {
                Some("succeeded") => Some(ProviderEvent::RefundSettled {
                    provider_reference: refund.id,
                }),
                Some("failed" | "canceled") => Some(ProviderEvent::RefundFailed {
                    provider_reference: refund.id,
                    reason: refund
                        .failure_reason
                        .unwrap_or_else(|| "refused by the bank".to_owned()),
                }),
                _ => None,
            })
        }
        _ => Ok(None),
    }
}

impl PaymentProvider for StripeProvider {
    /// Cards only: Stripe has no instalment plan of the shop's own.
    fn supports(&self, method: &PaymentMethod) -> bool {
        *method == PaymentMethod::Card
    }

    fn start<'a>(
        &'a self,
        payment: &'a PaymentView,
        session: Option<&'a str>,
        _urls: &'a ReturnUrls,
    ) -> ProviderFuture<'a, StartedPayment> {
        Box::pin(async move {
            if let Some(known) = session {
                let intent = self.retrieve_intent(known).await?;
                if intent.status != "canceled" {
                    return self.started(intent);
                }
                // Called off, yet the payment is still awaited: a fresh
                // intent, under a key of its own.
                let key = format!("{}:{known}", payment.id);
                return self.started(self.create_intent(payment, &key).await?);
            }
            // The payment's id is the key: two tabs open one intent.
            self.started(self.create_intent(payment, &payment.id).await?)
        })
    }

    fn cancel<'a>(&'a self, session: &'a str) -> ProviderFuture<'a, CancelOutcome> {
        Box::pin(async move {
            let cancelled: Result<Intent, CallError> = self
                .call(
                    self.http
                        .post(self.url(&format!("/v1/payment_intents/{session}/cancel"))),
                    None,
                )
                .await;
            let refusal = match cancelled {
                Ok(_) => return Ok(CancelOutcome::Cancelled),
                Err(CallError::Api(api))
                    if api.code.as_deref() == Some("payment_intent_unexpected_state") =>
                {
                    api
                }
                Err(err) => return Err(err.into()),
            };
            // Not cancellable: because it is paid, or already called off?
            let intent = self.retrieve_intent(session).await?;
            match intent.status.as_str() {
                "succeeded" => Ok(CancelOutcome::AlreadyPaid {
                    reference: intent.id,
                }),
                "canceled" => Ok(CancelOutcome::Cancelled),
                // `processing`: the bank has not answered; ask again later.
                _ => Err(ProviderError::Unavailable(refusal.message)),
            }
        })
    }

    fn refund<'a>(&'a self, refund: &'a ProviderRefund) -> ProviderFuture<'a, RefundOutcome> {
        Box::pin(async move {
            if !refund.psp_reference.starts_with("pi_") {
                return Err(ProviderError::Refused(format!(
                    "`{}` was not paid through Stripe",
                    refund.psp_reference
                )));
            }
            let form = [
                ("payment_intent", refund.psp_reference.clone()),
                ("amount", refund.amount.minor.to_string()),
            ];
            let made: Refund = self
                .call(
                    self.http.post(self.url("/v1/refunds")).form(&form),
                    Some(&refund.idempotency_key),
                )
                .await?;
            match made.status.as_deref() {
                Some("succeeded") => Ok(RefundOutcome::Settled { reference: made.id }),
                Some("failed" | "canceled") => Err(ProviderError::Refused(
                    made.failure_reason
                        .unwrap_or_else(|| "refused by the bank".to_owned()),
                )),
                // `pending`, `requires_action`: a webhook will tell.
                _ => Ok(RefundOutcome::Pending { reference: made.id }),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_webhook_is_only_trusted_with_a_fresh_matching_signature() {
        let payload = br#"{"type":"ping","data":{"object":{}}}"#;
        let header = sign_webhook("whsec_test", 1_000, payload);
        assert_eq!(
            verify_signature("whsec_test", &header, payload, 1_100),
            Ok(())
        );
        // A rotated secret sends two signatures: one match is enough.
        let rotated = format!("{header},v1={}", "00".repeat(32));
        assert_eq!(
            verify_signature("whsec_test", &rotated, payload, 1_100),
            Ok(())
        );

        assert_eq!(
            verify_signature("whsec_other", &header, payload, 1_100),
            Err(WebhookError::BadSignature)
        );
        assert_eq!(
            verify_signature("whsec_test", &header, b"{\"tampered\":true}", 1_100),
            Err(WebhookError::BadSignature)
        );
        assert_eq!(
            verify_signature("whsec_test", &header, payload, 1_000 + 301),
            Err(WebhookError::Stale)
        );
        for malformed in ["", "v1=abcd", "t=1000", "t=soon,v1=abcd"] {
            assert_eq!(
                verify_signature("whsec_test", malformed, payload, 1_000),
                Err(WebhookError::MalformedSignature),
                "{malformed}"
            );
        }
    }

    #[test]
    fn stripe_events_become_provider_events() {
        let paid = br#"{"type":"payment_intent.succeeded","data":{"object":{
            "id":"pi_1","amount_received":12585,"currency":"eur",
            "metadata":{"payment_id":"pay-1","order_id":"ord-1"}}}}"#;
        assert_eq!(
            read_event(paid),
            Ok(Some(ProviderEvent::Paid {
                payment_id: "pay-1".into(),
                reference: "pi_1".into(),
                amount: Money::eur(12_585),
            }))
        );
        // An intent that is not the shop's.
        let foreign = br#"{"type":"payment_intent.succeeded","data":{"object":{
            "id":"pi_2","amount_received":100,"currency":"eur","metadata":{}}}}"#;
        assert_eq!(read_event(foreign), Ok(None));

        let settled =
            br#"{"type":"refund.updated","data":{"object":{"id":"re_1","status":"succeeded"}}}"#;
        assert_eq!(
            read_event(settled),
            Ok(Some(ProviderEvent::RefundSettled {
                provider_reference: "re_1".into()
            }))
        );
        let failed = br#"{"type":"refund.failed","data":{"object":{
            "id":"re_2","status":"failed","failure_reason":"expired_or_canceled_card"}}}"#;
        assert_eq!(
            read_event(failed),
            Ok(Some(ProviderEvent::RefundFailed {
                provider_reference: "re_2".into(),
                reason: "expired_or_canceled_card".into(),
            }))
        );
        let pending =
            br#"{"type":"refund.updated","data":{"object":{"id":"re_3","status":"pending"}}}"#;
        assert_eq!(read_event(pending), Ok(None));
        // A failed card payment is no event: the shopper tries again.
        let declined =
            br#"{"type":"payment_intent.payment_failed","data":{"object":{"id":"pi_3"}}}"#;
        assert_eq!(read_event(declined), Ok(None));
        assert!(matches!(
            read_event(b"not json"),
            Err(WebhookError::Payload(_))
        ));
    }

    #[test]
    fn the_secrets_stay_out_of_debug_output() {
        let config = StripeConfig::new("sk_live_secret", "pk_live_public", "whsec_secret");
        let shown = format!("{config:?}");
        assert!(shown.contains("pk_live_public"));
        assert!(!shown.contains("sk_live_secret") && !shown.contains("whsec_secret"));
    }
}
