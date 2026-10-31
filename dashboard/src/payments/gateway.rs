//! What the dashboard asks of a payment provider, behind a trait so tests
//! run without one. [`StripeGateway`] speaks to Stripe through async-stripe,
//! with the tenant's own secret key on every call.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Mutex;

use stripe::{ClientBuilder, IdempotencyKey, RequestStrategy, StripeRequest};
use stripe_core::PaymentIntentStatus;
use stripe_core::balance::RetrieveForMyAccountBalance;
use stripe_core::payment_intent::{
    CancelPaymentIntent, CancelPaymentIntentCancellationReason, CreatePaymentIntent,
    CreatePaymentIntentAutomaticPaymentMethods, RetrievePaymentIntent,
};
use stripe_core::refund::{CreateRefund, ListRefund};

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

/// A refund of a payment as the provider holds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefundRecord {
    pub refund_id: String,
    /// Minor units of the payment's currency.
    pub amount_cents: u64,
    /// Whether the money is on its way back (as opposed to pending, failed
    /// or cancelled).
    pub succeeded: bool,
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

/// What came of asking the provider to cancel a payment the shopper has not
/// confirmed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cancellation {
    /// The payment is cancelled (now, or it already was): the shopper's
    /// card form cannot confirm it any more, and nothing was charged.
    Cancelled,
    /// The shopper is confirming it right now (a bank's challenge, a
    /// payment method that settles later): left alone, the webhook settles
    /// it.
    Confirming,
    /// The shopper paid: the webhook turns it into the order, or gives it
    /// back.
    Paid,
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
    /// The refunds the provider holds for the payment, the ones made by this
    /// server and the ones made in the provider's own dashboard alike.
    async fn refunds_of(
        &self,
        secret_key: &str,
        payment_id: &str,
    ) -> anyhow::Result<Vec<RefundRecord>>;
    /// Cancels the payment unless the shopper confirmed it: the answer says
    /// which. Asked twice, the second time answers `Cancelled` again.
    async fn cancel_payment(
        &self,
        secret_key: &str,
        payment_id: &str,
    ) -> anyhow::Result<Cancellation>;
}

/// Reads [`Cancellation`] off a payment's status, for one the shopper may
/// confirm no more; `None` while it may still be cancelled.
fn settled(status: &PaymentIntentStatus) -> Option<Cancellation> {
    match status {
        PaymentIntentStatus::Canceled => Some(Cancellation::Cancelled),
        PaymentIntentStatus::Succeeded => Some(Cancellation::Paid),
        PaymentIntentStatus::Processing
        | PaymentIntentStatus::RequiresAction
        | PaymentIntentStatus::RequiresCapture => Some(Cancellation::Confirming),
        // A status this crate does not know is tried as well: Stripe
        // refuses the cancel itself when it must.
        _ => None,
    }
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

/// Sends a request, its future boxed and its error already anyhow's.
///
/// Awaiting async-stripe's `send` straight from the `async_trait` methods
/// below cost about forty seconds of type checking and as much borrow
/// checking per method, two thirds of the crate's compile time. Proving the
/// future `Send` proves the response type `Send`, and a Stripe response type
/// reaches the whole Stripe object graph; rustc redoes that proof in every
/// function that needs it. So the request is sent for a JSON value, a type
/// the proof settles at once, and the response is read from the value only
/// after the await, where its type no longer matters to the future. The
/// value round-trips through text because miniserde reads a type from text
/// alone, a few kilobytes per call.
fn send<'a, R>(
    request: &R,
    strategy: Option<RequestStrategy>,
    client: &'a stripe::Client,
) -> Pin<Box<dyn Future<Output = anyhow::Result<R::Output>> + Send + 'a>>
where
    R: StripeRequest,
    R::Output: miniserde::Deserialize + 'static,
{
    let mut request = request.build().customize::<miniserde::json::Value>();
    if let Some(strategy) = strategy {
        request = request.request_strategy(strategy);
    }
    Box::pin(async move {
        let value = request
            .send(client)
            .await
            .map_err(|err| anyhow::anyhow!("{err}"))?;
        miniserde::json::from_str(&miniserde::json::to_string(&value))
            .map_err(|err| anyhow::anyhow!("Stripe answered with an unexpected body: {err}"))
    })
}

#[async_trait::async_trait]
impl PaymentGateway for StripeGateway {
    async fn check_key(&self, secret_key: &str) -> anyhow::Result<()> {
        let client = self.client(secret_key)?;
        send(&RetrieveForMyAccountBalance::new(), None, &client).await?;
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
        let client = self.client(secret_key)?;
        let strategy = idempotent(&payment.idempotency_key)?;
        let intent = send(&request, Some(strategy), &client).await?;
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
        let client = self.client(secret_key)?;
        let intent = send(&RetrievePaymentIntent::new(payment_id), None, &client).await?;
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
        let client = self.client(secret_key)?;
        let request = CreateRefund::new()
            .payment_intent(payment_id)
            .amount(amount_cents as i64);
        let refund = send(&request, Some(idempotent(idempotency_key)?), &client).await?;
        Ok(refund.id.to_string())
    }

    async fn refunds_of(
        &self,
        secret_key: &str,
        payment_id: &str,
    ) -> anyhow::Result<Vec<RefundRecord>> {
        let client = self.client(secret_key)?;
        let request = ListRefund::new().payment_intent(payment_id).limit(100);
        let refunds = send(&request, None, &client).await?;
        Ok(refunds
            .data
            .into_iter()
            .map(|refund| RefundRecord {
                refund_id: refund.id.to_string(),
                amount_cents: refund.amount.max(0) as u64,
                succeeded: refund.status.as_deref() == Some("succeeded"),
            })
            .collect())
    }

    /// Stripe cancels a payment only while nobody confirmed it, so its
    /// status is read first; a confirmation that lands between the two
    /// calls fails the cancel, and the status is read again to say so.
    async fn cancel_payment(
        &self,
        secret_key: &str,
        payment_id: &str,
    ) -> anyhow::Result<Cancellation> {
        let client = self.client(secret_key)?;
        let intent = send(&RetrievePaymentIntent::new(payment_id), None, &client).await?;
        if let Some(settled) = settled(&intent.status) {
            return Ok(settled);
        }
        let cancel = CancelPaymentIntent::new(payment_id)
            .cancellation_reason(CancelPaymentIntentCancellationReason::Abandoned);
        let refused = match send(&cancel, None, &client).await {
            Ok(_) => return Ok(Cancellation::Cancelled),
            Err(err) => err,
        };
        let intent = send(&RetrievePaymentIntent::new(payment_id), None, &client).await?;
        settled(&intent.status).ok_or(refused)
    }
}
