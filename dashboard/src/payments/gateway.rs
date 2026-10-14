//! What the dashboard asks of a payment provider, behind a trait so tests
//! run without one. [`StripeGateway`] speaks to Stripe through async-stripe,
//! with the tenant's own secret key on every call.

use std::collections::HashMap;
use std::sync::Mutex;

use stripe::{ClientBuilder, IdempotencyKey, RequestStrategy, StripeRequest};
use stripe_core::PaymentIntentStatus;
use stripe_core::balance::RetrieveForMyAccountBalance;
use stripe_core::payment_intent::{
    CreatePaymentIntent, CreatePaymentIntentAutomaticPaymentMethods, RetrievePaymentIntent,
};
use stripe_core::refund::CreateRefund;

/// A payment to open for a cart.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewPayment {
    /// Minor units of `currency`.
    pub amount_cents: u64,
    /// ISO-4217, any case.
    pub currency: String,
    /// The same key opens the same payment: a request answered twice (two
    /// nodes, a retry) charges once.
    pub idempotency_key: String,
    /// Where the payment came from, read back when it succeeds.
    pub metadata: HashMap<String, String>,
    pub receipt_email: Option<String>,
    pub description: Option<String>,
}

/// A payment the provider opened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenPayment {
    pub payment_id: String,
    pub client_secret: String,
}

/// A payment as the provider sees it now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaymentStatus {
    pub payment_id: String,
    pub succeeded: bool,
    /// What was received, in minor units of `currency`.
    pub amount_received: u64,
    /// ISO-4217, lowercase as Stripe writes it.
    pub currency: String,
    pub metadata: HashMap<String, String>,
}

#[async_trait::async_trait]
pub trait PaymentGateway: Send + Sync {
    /// Whether the secret key works (a cheap authenticated call).
    async fn check_key(&self, secret_key: &str) -> anyhow::Result<()>;
    async fn open_payment(
        &self,
        secret_key: &str,
        payment: NewPayment,
    ) -> anyhow::Result<OpenPayment>;
    async fn payment_status(
        &self,
        secret_key: &str,
        payment_id: &str,
    ) -> anyhow::Result<PaymentStatus>;
    /// Gives `amount_cents` of the payment back; returns the refund's id.
    /// The same key refunds once.
    async fn refund(
        &self,
        secret_key: &str,
        payment_id: &str,
        amount_cents: u64,
        idempotency_key: &str,
    ) -> anyhow::Result<String>;
}

/// Stripe, through async-stripe. One client per secret key, kept.
pub struct StripeGateway {
    api_url: Option<String>,
    clients: Mutex<HashMap<String, stripe::Client>>,
}

impl StripeGateway {
    /// `api_url`: Stripe's own address when `None`.
    pub fn new(api_url: Option<String>) -> Self {
        // Two TLS backends are compiled in (the store API's and the HTTP
        // clients'), so rustls cannot pick one by itself: name it, as the
        // store API does. A provider installed already stays.
        let _ = tokio_rustls::rustls::crypto::aws_lc_rs::default_provider().install_default();
        Self {
            api_url,
            clients: Mutex::default(),
        }
    }

    fn client(&self, secret_key: &str) -> anyhow::Result<stripe::Client> {
        let mut clients = self
            .clients
            .lock()
            .map_err(|_| anyhow::anyhow!("the Stripe clients lock is poisoned"))?;
        if let Some(client) = clients.get(secret_key) {
            return Ok(client.clone());
        }
        let mut builder = ClientBuilder::new(secret_key);
        if let Some(url) = &self.api_url {
            builder = builder.url(url.clone());
        }
        let client = builder.build()?;
        clients.insert(secret_key.to_owned(), client.clone());
        Ok(client)
    }
}

fn idempotent(key: &str) -> anyhow::Result<RequestStrategy> {
    Ok(RequestStrategy::Idempotent(
        IdempotencyKey::new(key).map_err(|err| anyhow::anyhow!("{err:?}"))?,
    ))
}

#[async_trait::async_trait]
impl PaymentGateway for StripeGateway {
    async fn check_key(&self, secret_key: &str) -> anyhow::Result<()> {
        RetrieveForMyAccountBalance::new()
            .send(&self.client(secret_key)?)
            .await?;
        Ok(())
    }

    async fn open_payment(
        &self,
        secret_key: &str,
        payment: NewPayment,
    ) -> anyhow::Result<OpenPayment> {
        let currency: stripe_types::Currency =
            payment.currency.to_ascii_lowercase().parse().map_err(|_| {
                anyhow::anyhow!("Stripe does not know the currency {}", payment.currency)
            })?;
        let mut request = CreatePaymentIntent::new(payment.amount_cents as i64, currency)
            .automatic_payment_methods(CreatePaymentIntentAutomaticPaymentMethods::new(true))
            .metadata(payment.metadata);
        if let Some(email) = payment.receipt_email {
            request = request.receipt_email(email);
        }
        if let Some(description) = payment.description {
            request = request.description(description);
        }
        let intent = request
            .customize()
            .request_strategy(idempotent(&payment.idempotency_key)?)
            .send(&self.client(secret_key)?)
            .await?;
        Ok(OpenPayment {
            payment_id: intent.id.to_string(),
            client_secret: intent.client_secret.ok_or_else(|| {
                anyhow::anyhow!("Stripe opened a payment without a client secret")
            })?,
        })
    }

    async fn payment_status(
        &self,
        secret_key: &str,
        payment_id: &str,
    ) -> anyhow::Result<PaymentStatus> {
        let intent = RetrievePaymentIntent::new(payment_id)
            .send(&self.client(secret_key)?)
            .await?;
        Ok(PaymentStatus {
            payment_id: intent.id.to_string(),
            succeeded: intent.status == PaymentIntentStatus::Succeeded,
            amount_received: intent.amount_received.max(0) as u64,
            currency: intent.currency.to_string(),
            metadata: intent.metadata,
        })
    }

    async fn refund(
        &self,
        secret_key: &str,
        payment_id: &str,
        amount_cents: u64,
        idempotency_key: &str,
    ) -> anyhow::Result<String> {
        let refund = CreateRefund::new()
            .payment_intent(payment_id)
            .amount(amount_cents as i64)
            .customize()
            .request_strategy(idempotent(idempotency_key)?)
            .send(&self.client(secret_key)?)
            .await?;
        Ok(refund.id.to_string())
    }
}
