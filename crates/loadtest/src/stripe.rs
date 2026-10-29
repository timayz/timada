//! A stand-in for Stripe's API, ported from the browser suite's
//! (tests/e2e/stripe.ts): every call goes on to stripe-mock, so the
//! dashboard reads answers shaped exactly as Stripe's, and what stripe-mock
//! forgets is kept here (the amount and metadata a payment was opened with,
//! the idempotency key that opened it, whether the shopper paid). Each call
//! waits `latency` first, as a call across the internet would.
//!
//! Paying also does what Stripe does next: it tells the dashboard through
//! the shop's webhook, a moment later, retrying while the dashboard asks to,
//! and now and then twice, since Stripe delivers at least once.

use std::collections::{BTreeMap, HashMap};
use std::convert::Infallible;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::Result;
use hmac::{Hmac, Mac};
use http_body_util::{BodyExt, Full};
use hyper::body::{Bytes, Incoming};
use hyper::{Request, Response};
use rand::Rng;
use serde_json::{Value, json};
use sha2::Sha256;
use tokio::net::TcpListener;

use crate::metrics::Metrics;

/// Deliveries of one webhook before Stripe gives up on it.
const WEBHOOK_ATTEMPTS: u32 = 6;
/// How often a payment's webhook is delivered a second time.
const DUPLICATE_DELIVERY: f64 = 0.05;

struct Intent {
    amount: u64,
    currency: String,
    metadata: BTreeMap<String, String>,
    paid: bool,
}

pub struct FakeStripe {
    intents: Mutex<HashMap<String, Intent>>,
    by_key: Mutex<HashMap<String, String>>,
    upstream: String,
    client: reqwest::Client,
    latency: Duration,
    webhook_base: String,
    webhook_secret: String,
    metrics: Arc<Metrics>,
}

pub struct Settings {
    /// stripe-mock.
    pub upstream: String,
    pub latency: Duration,
    /// The dashboard, where `/stripe/webhooks/{tenant}` is.
    pub webhook_base: String,
    pub webhook_secret: String,
}

impl FakeStripe {
    pub fn new(settings: Settings, client: reqwest::Client, metrics: Arc<Metrics>) -> Arc<Self> {
        Arc::new(Self {
            intents: Mutex::default(),
            by_key: Mutex::default(),
            upstream: settings.upstream.trim_end_matches('/').to_owned(),
            client,
            latency: settings.latency,
            webhook_base: settings.webhook_base.trim_end_matches('/').to_owned(),
            webhook_secret: settings.webhook_secret,
            metrics,
        })
    }

    /// Answers the dashboard's calls on `listener` until the process ends.
    pub fn serve(self: &Arc<Self>, listener: TcpListener) {
        let stripe = self.clone();
        tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    continue;
                };
                let stripe = stripe.clone();
                tokio::spawn(async move {
                    let service = hyper::service::service_fn(move |request| {
                        let stripe = stripe.clone();
                        async move { Ok::<_, Infallible>(stripe.answer(request).await) }
                    });
                    let io = hyper_util::rt::TokioIo::new(stream);
                    let _ = hyper::server::conn::http1::Builder::new()
                        .serve_connection(io, service)
                        .await;
                });
            }
        });
    }

    async fn answer(&self, request: Request<Incoming>) -> Response<Full<Bytes>> {
        let (status, json) = match self.handle(request).await {
            Ok(answer) => answer,
            Err(err) => (500, json!({ "error": { "message": err.to_string() } })),
        };
        Response::builder()
            .status(status)
            .header("content-type", "application/json")
            .body(Full::new(Bytes::from(json.to_string())))
            .expect("a valid response")
    }

    async fn handle(&self, request: Request<Incoming>) -> Result<(u16, Value)> {
        let jitter = rand::rng().random_range(0.5..1.5);
        tokio::time::sleep(self.latency.mul_f64(jitter)).await;
        let method = request.method().clone();
        let uri = request.uri().clone();
        let header = |name: &str| {
            request
                .headers()
                .get(name)
                .and_then(|value| value.to_str().ok())
                .map(str::to_owned)
        };
        let authorization = header("authorization").unwrap_or_else(|| "Bearer sk_test_123".into());
        let key = header("idempotency-key");
        let body = String::from_utf8(request.into_body().collect().await?.to_bytes().to_vec())?;

        let path = uri.path();
        let path_and_query = uri.path_and_query().map_or(path, |pq| pq.as_str());
        let mut forward = self
            .client
            .request(method.clone(), format!("{}{path_and_query}", self.upstream))
            .header("authorization", authorization);
        if method != hyper::Method::GET {
            forward = forward
                .header("content-type", "application/x-www-form-urlencoded")
                .body(body.clone());
        }
        let started = Instant::now();
        let answer = forward.send().await;
        self.metrics.request(
            &format!("stripe-mock {method} {}", stripe_route(path)),
            started.elapsed(),
            answer.as_ref().err().map(|_| "transport".into()),
        );
        let answer = answer?;
        let status = answer.status().as_u16();
        let json: Value = answer.json().await?;

        if method == hyper::Method::POST && path == "/v1/payment_intents" {
            let form = decode_form(&body);
            let known = key
                .as_ref()
                .and_then(|key| self.by_key.lock().unwrap().get(key).cloned());
            let id = match known {
                Some(id) => id,
                None => {
                    let id = json["id"].as_str().unwrap_or_default().to_owned();
                    let metadata = form
                        .iter()
                        .filter_map(|(name, value)| {
                            let name = name.strip_prefix("metadata[")?.strip_suffix(']')?;
                            Some((name.to_owned(), value.clone()))
                        })
                        .collect();
                    let amount = form
                        .iter()
                        .find(|(name, _)| name == "amount")
                        .and_then(|(_, value)| value.parse().ok())
                        .unwrap_or(0);
                    let currency = form
                        .iter()
                        .find(|(name, _)| name == "currency")
                        .map(|(_, value)| value.clone())
                        .unwrap_or_default();
                    self.intents.lock().unwrap().insert(
                        id.clone(),
                        Intent {
                            amount,
                            currency,
                            metadata,
                            paid: false,
                        },
                    );
                    if let Some(key) = key {
                        self.by_key.lock().unwrap().insert(key, id.clone());
                    }
                    id
                }
            };
            return Ok((200, self.patch(json, &id)));
        }
        if let Some(id) = path.strip_prefix("/v1/payment_intents/")
            && method == hyper::Method::GET
            && !id.contains('/')
        {
            return Ok((200, self.patch(json, id)));
        }
        if method == hyper::Method::POST && path == "/v1/refunds" {
            let intent = decode_form(&body)
                .into_iter()
                .find(|(name, _)| name == "payment_intent")
                .map(|(_, value)| value)
                .unwrap_or_default();
            // Nobody refunds in these runs: the dashboard refunds a paid
            // payment only when it cannot place its order.
            self.metrics
                .anomaly("refund", format!("the dashboard refunded {intent}"));
        }
        Ok((status, json))
    }

    fn patch(&self, mut json: Value, id: &str) -> Value {
        let intents = self.intents.lock().unwrap();
        let Some(intent) = intents.get(id) else {
            return json;
        };
        json["id"] = json!(id);
        json["client_secret"] = json!(format!("{id}_secret_load"));
        json["amount"] = json!(intent.amount);
        json["amount_received"] = json!(if intent.paid { intent.amount } else { 0 });
        // stripe-mock answers with its fixture's currency.
        json["currency"] = json!(intent.currency);
        json["metadata"] = json!(intent.metadata);
        json["status"] = json!(if intent.paid {
            "succeeded"
        } else {
            "requires_payment_method"
        });
        json
    }

    /// The payment a client secret is for, as Stripe.js would know it; the
    /// amount it was opened with.
    pub fn amount_of(&self, client_secret: &str) -> Option<u64> {
        let id = client_secret.split("_secret_").next()?;
        self.intents
            .lock()
            .unwrap()
            .get(id)
            .map(|intent| intent.amount)
    }

    /// The shopper paid: marks it so, then tells the dashboard. `false`
    /// when no payment has that client secret.
    pub fn pay(self: &Arc<Self>, client_secret: &str) -> bool {
        let Some(id) = client_secret.split("_secret_").next().map(str::to_owned) else {
            return false;
        };
        let tenant = {
            let mut intents = self.intents.lock().unwrap();
            let Some(intent) = intents.get_mut(&id) else {
                return false;
            };
            intent.paid = true;
            intent
                .metadata
                .get("tenant_id")
                .cloned()
                .unwrap_or_default()
        };
        let duplicate = rand::rng().random_bool(DUPLICATE_DELIVERY);
        for delivery in 0..(1 + duplicate as u32) {
            let stripe = self.clone();
            let (id, tenant) = (id.clone(), tenant.clone());
            tokio::spawn(async move {
                let delay = rand::rng().random_range(50..1500) + delivery as u64 * 3000;
                tokio::time::sleep(Duration::from_millis(delay)).await;
                stripe.deliver(&tenant, &id).await;
            });
        }
        true
    }

    async fn deliver(&self, tenant: &str, id: &str) {
        let body = json!({
            "id": format!("evt_{id}"),
            "type": "payment_intent.succeeded",
            "data": { "object": { "id": id, "object": "payment_intent" } },
        })
        .to_string();
        let url = format!("{}/stripe/webhooks/{tenant}", self.webhook_base);
        let route = "POST /stripe/webhooks/{tenant}";
        for attempt in 0..WEBHOOK_ATTEMPTS {
            let started = Instant::now();
            let answer = self
                .client
                .post(&url)
                .header("content-type", "application/json")
                .header("stripe-signature", self.signature(&body))
                .body(body.clone())
                .send()
                .await;
            let took = started.elapsed();
            match answer.map(|answer| answer.status().as_u16()) {
                Ok(204) => {
                    self.metrics.request(route, took, None);
                    return;
                }
                Ok(status) => {
                    self.metrics.request(route, took, Some(status.to_string()));
                    // A refusal Stripe would not retry: the dashboard
                    // turned a genuine payment away.
                    if status < 500 {
                        self.metrics
                            .anomaly("webhook_refused", format!("{id}: {status}"));
                        return;
                    }
                }
                Err(_) => self.metrics.request(route, took, Some("transport".into())),
            }
            self.metrics.count("webhook.retry");
            tokio::time::sleep(Duration::from_secs(1 << attempt)).await;
        }
        self.metrics.anomaly(
            "webhook_gave_up",
            format!("{id}: {WEBHOOK_ATTEMPTS} attempts"),
        );
    }

    /// The `Stripe-Signature` header Stripe would send with `body`.
    fn signature(&self, body: &str) -> String {
        let t = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let mut mac =
            Hmac::<Sha256>::new_from_slice(self.webhook_secret.as_bytes()).expect("any key length");
        mac.update(format!("{t}.{body}").as_bytes());
        format!("t={t},v1={}", hex::encode(mac.finalize().into_bytes()))
    }
}

fn stripe_route(path: &str) -> &str {
    match path {
        p if p.starts_with("/v1/payment_intents/") => "/v1/payment_intents/{id}",
        p => p,
    }
}

fn decode_form(body: &str) -> Vec<(String, String)> {
    body.split('&')
        .filter_map(|pair| pair.split_once('='))
        .map(|(name, value)| (decode(name), decode(value)))
        .collect()
}

fn decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("");
                match u8::from_str_radix(hex, 16) {
                    Ok(byte) => {
                        out.push(byte);
                        i += 2;
                    }
                    Err(_) => out.push(b'%'),
                }
            }
            byte => out.push(byte),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_form_body_decodes() {
        assert_eq!(
            decode_form("amount=2500&metadata%5Bcart_id%5D=ab+c%2F&x=%"),
            vec![
                ("amount".into(), "2500".into()),
                ("metadata[cart_id]".into(), "ab c/".into()),
                ("x".into(), "%".into()),
            ]
        );
    }
}
