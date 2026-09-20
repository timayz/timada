//! The Stripe adapter against a local stand-in for Stripe's API: what it
//! sends (keys, amounts, idempotency keys) and what it makes of the answers.
#![cfg(feature = "stripe")]

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use axum::{
    Form, Json, Router,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    routing::{get, post},
};
use serde_json::{Value, json};
use timada_core::Money;
use timada_payment::{
    CancelOutcome, Command, PaymentMethod, PaymentProvider, PaymentStart, ProviderError,
    ProviderRefund, RefundOutcome, RequestPayment, ReturnUrls, StripeConfig, StripeProvider,
    load_payment, migrations, start_payment,
};

/// What the stand-in remembers and how it is told to answer.
#[derive(Default)]
struct Fake {
    /// Intent id → status.
    intents: HashMap<String, String>,
    /// Idempotency key → intent id.
    intent_keys: HashMap<String, String>,
    created: Vec<HashMap<String, String>>,
    refunds: Vec<(String, HashMap<String, String>)>,
    /// `(HTTP status, body)` of the next refund; a succeeded refund otherwise.
    refund_answers: Vec<(u16, Value)>,
    authorizations: Vec<String>,
}

type Shared = Arc<Mutex<Fake>>;

fn lock(fake: &Shared) -> std::sync::MutexGuard<'_, Fake> {
    fake.lock().unwrap_or_else(|e| e.into_inner())
}

fn intent(id: &str, status: &str) -> Value {
    json!({ "id": id, "status": status, "client_secret": format!("{id}_secret_x") })
}

fn note_auth(fake: &mut Fake, headers: &HeaderMap) {
    let auth = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    fake.authorizations.push(auth.to_owned());
}

fn key_of(headers: &HeaderMap) -> String {
    headers
        .get("idempotency-key")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_owned()
}

async fn create_intent(
    State(fake): State<Shared>,
    headers: HeaderMap,
    Form(form): Form<HashMap<String, String>>,
) -> Json<Value> {
    let mut fake = lock(&fake);
    note_auth(&mut fake, &headers);
    let key = key_of(&headers);
    if let Some(id) = fake.intent_keys.get(&key).cloned() {
        let status = fake.intents.get(&id).cloned().unwrap_or_default();
        return Json(intent(&id, &status));
    }
    let id = format!("pi_{}", fake.intents.len() + 1);
    fake.intents
        .insert(id.clone(), "requires_payment_method".to_owned());
    fake.intent_keys.insert(key, id.clone());
    fake.created.push(form);
    Json(intent(&id, "requires_payment_method"))
}

async fn retrieve_intent(State(fake): State<Shared>, Path(id): Path<String>) -> Json<Value> {
    let status = lock(&fake).intents.get(&id).cloned().unwrap_or_default();
    Json(intent(&id, &status))
}

async fn cancel_intent(
    State(fake): State<Shared>,
    Path(id): Path<String>,
) -> (StatusCode, Json<Value>) {
    let mut fake = lock(&fake);
    let status = fake.intents.get(&id).cloned().unwrap_or_default();
    if status == "requires_payment_method" {
        fake.intents.insert(id.clone(), "canceled".to_owned());
        return (StatusCode::OK, Json(intent(&id, "canceled")));
    }
    let error = json!({ "error": {
        "code": "payment_intent_unexpected_state",
        "message": format!("This PaymentIntent has a status of {status}."),
    }});
    (StatusCode::BAD_REQUEST, Json(error))
}

async fn create_refund(
    State(fake): State<Shared>,
    headers: HeaderMap,
    Form(form): Form<HashMap<String, String>>,
) -> (StatusCode, Json<Value>) {
    let mut fake = lock(&fake);
    fake.refunds.push((key_of(&headers), form));
    if fake.refund_answers.is_empty() {
        let id = format!("re_{}", fake.refunds.len());
        return (
            StatusCode::OK,
            Json(json!({ "id": id, "status": "succeeded" })),
        );
    }
    let (status, body) = fake.refund_answers.remove(0);
    (
        StatusCode::from_u16(status).unwrap_or(StatusCode::OK),
        Json(body),
    )
}

/// Starts the stand-in on a free local port.
async fn stripe() -> anyhow::Result<(StripeProvider, Shared)> {
    let fake = Shared::default();
    let app = Router::new()
        .route("/v1/payment_intents", post(create_intent))
        .route("/v1/payment_intents/{id}", get(retrieve_intent))
        .route("/v1/payment_intents/{id}/cancel", post(cancel_intent))
        .route("/v1/refunds", post(create_refund))
        .with_state(fake.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    tokio::spawn(async move {
        if let Err(err) = axum::serve(listener, app).await {
            tracing::error!(%err, "stand-in stopped");
        }
    });
    let mut config = StripeConfig::new("sk_test_123", "pk_test_123", "whsec_123");
    config.api_base = format!("http://{address}");
    Ok((StripeProvider::new(config)?, fake))
}

fn refund(reference: &str, cents: i64, key: &str) -> ProviderRefund {
    ProviderRefund {
        psp_reference: reference.into(),
        amount: Money::eur(cents),
        idempotency_key: key.into(),
    }
}

#[tokio::test]
async fn one_intent_per_payment_reused_and_replaced_once_cancelled() -> anyhow::Result<()> {
    let (provider, fake) = stripe().await?;
    let (executor, db) = timada_core::testing::memory_executor(migrations()).await?;
    let urls = ReturnUrls {
        paid: "https://shop.test/checkout/pay/1".into(),
    };
    let id = Command(&executor)
        .request_payment(RequestPayment {
            order_id: "order-stripe".into(),
            amount: Money::eur(12_585),
            method: PaymentMethod::Card,
        })
        .await?;

    assert!(provider.supports(&PaymentMethod::Card));
    assert!(!provider.supports(&PaymentMethod::Installments {
        count: 3,
        fee: Money::eur(0)
    }));

    let first = start_payment(&executor, &db, &provider, &id, &urls).await?;
    assert_eq!(
        first,
        PaymentStart::ClientSecret {
            client_secret: "pi_1_secret_x".into(),
            publishable_key: "pk_test_123".into(),
        }
    );
    // A second visit reads the same intent back: nothing new is created.
    assert_eq!(
        start_payment(&executor, &db, &provider, &id, &urls).await?,
        first
    );
    {
        let fake = lock(&fake);
        assert_eq!(fake.created.len(), 1);
        let sent = &fake.created[0];
        assert_eq!(sent["amount"], "12585");
        assert_eq!(sent["currency"], "eur");
        assert_eq!(sent["payment_method_types[]"], "card");
        assert_eq!(sent["metadata[payment_id]"], id);
        assert_eq!(sent["metadata[order_id]"], "order-stripe");
        assert_eq!(fake.intent_keys.get(&id).map(String::as_str), Some("pi_1"));
        // The secret key, as HTTP basic auth: base64("sk_test_123:").
        assert!(
            fake.authorizations
                .iter()
                .all(|a| a == "Basic c2tfdGVzdF8xMjM6"),
            "{:?}",
            fake.authorizations
        );
    }

    // Called off while the payment is still awaited: a fresh intent.
    assert_eq!(provider.cancel("pi_1").await?, CancelOutcome::Cancelled);
    assert_eq!(provider.cancel("pi_1").await?, CancelOutcome::Cancelled);
    let again = start_payment(&executor, &db, &provider, &id, &urls).await?;
    assert!(
        matches!(&again, PaymentStart::ClientSecret { client_secret, .. } if client_secret == "pi_2_secret_x"),
        "{again:?}"
    );

    // Paid: too late to call off, and the reference to capture comes back.
    lock(&fake)
        .intents
        .insert("pi_2".into(), "succeeded".into());
    assert_eq!(
        provider.cancel("pi_2").await?,
        CancelOutcome::AlreadyPaid {
            reference: "pi_2".into()
        }
    );
    // The bank has not answered yet: neither — ask again later.
    lock(&fake)
        .intents
        .insert("pi_2".into(), "processing".into());
    assert!(matches!(
        provider.cancel("pi_2").await,
        Err(ProviderError::Unavailable(_))
    ));
    assert!(load_payment(&executor, &id).await?.is_some());
    Ok(())
}

#[tokio::test]
async fn refunds_are_keyed_and_stripes_answers_are_told_apart() -> anyhow::Result<()> {
    let (provider, fake) = stripe().await?;

    assert_eq!(
        provider.refund(&refund("pi_1", 2_000, "evt-1")).await?,
        RefundOutcome::Settled {
            reference: "re_1".into()
        }
    );
    {
        let fake = lock(&fake);
        let (key, sent) = &fake.refunds[0];
        assert_eq!(key, "evt-1");
        assert_eq!(sent["payment_intent"], "pi_1");
        assert_eq!(sent["amount"], "2000");
    }

    // Taken, not done yet: a webhook will tell.
    lock(&fake)
        .refund_answers
        .push((200, json!({ "id": "re_slow", "status": "pending" })));
    assert_eq!(
        provider.refund(&refund("pi_1", 100, "evt-2")).await?,
        RefundOutcome::Pending {
            reference: "re_slow".into()
        }
    );

    // Stripe says no — for good.
    lock(&fake).refund_answers.push((
        400,
        json!({ "error": { "code": "charge_already_refunded", "message": "Charge has already been refunded." } }),
    ));
    assert_eq!(
        provider.refund(&refund("pi_1", 100, "evt-3")).await,
        Err(ProviderError::Refused(
            "Charge has already been refunded.".into()
        ))
    );
    lock(&fake).refund_answers.push((
        200,
        json!({ "id": "re_x", "status": "failed", "failure_reason": "expired_or_canceled_card" }),
    ));
    assert_eq!(
        provider.refund(&refund("pi_1", 100, "evt-4")).await,
        Err(ProviderError::Refused("expired_or_canceled_card".into()))
    );

    // Stripe is having a bad moment — to try again.
    for status in [429, 500, 503] {
        lock(&fake)
            .refund_answers
            .push((status, json!({ "error": { "message": "try later" } })));
        assert_eq!(
            provider.refund(&refund("pi_1", 100, "evt-5")).await,
            Err(ProviderError::Unavailable("try later".into())),
            "{status}"
        );
    }

    // A payment captured by hand was never Stripe's to refund: not even sent.
    let before = lock(&fake).refunds.len();
    assert!(matches!(
        provider
            .refund(&refund("manual-order-1", 100, "evt-6"))
            .await,
        Err(ProviderError::Refused(_))
    ));
    assert_eq!(lock(&fake).refunds.len(), before);
    Ok(())
}

#[tokio::test]
async fn an_unreachable_stripe_is_unavailable_not_refused() -> anyhow::Result<()> {
    let mut config = StripeConfig::new("sk_test_123", "pk_test_123", "whsec_123");
    // Nothing listens there.
    config.api_base = "http://127.0.0.1:9".into();
    let provider = StripeProvider::new(config)?;
    assert!(matches!(
        provider.refund(&refund("pi_1", 100, "evt-1")).await,
        Err(ProviderError::Unavailable(_))
    ));
    Ok(())
}
