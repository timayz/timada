//! What the dashboard asks of a payment provider, behind a trait so tests
//! run without one. [`StripeGateway`] speaks to Stripe through async-stripe,
//! with the tenant's own secret key on every call.
//!
//! A payment is opened to be held on the shopper's card only (manual
//! capture): the money is taken later ([`PaymentGateway::capture_payment`])
//! or the hold given back ([`PaymentGateway::release_payment`]). Neither is
//! sent with an idempotency key: Stripe replays the first answer to a key,
//! errors included, for a day, while a payment's own state already makes
//! each of them happen once.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Mutex;

use stripe::{ClientBuilder, IdempotencyKey, RequestStrategy, StripeError, StripeRequest};
use stripe_core::balance::RetrieveForMyAccountBalance;
use stripe_core::payment_intent::{
    CancelPaymentIntent, CancelPaymentIntentCancellationReason, CapturePaymentIntent,
    CreatePaymentIntent, CreatePaymentIntentAutomaticPaymentMethods, RetrievePaymentIntent,
};
use stripe_core::refund::{CreateRefund, ListRefund};
use stripe_core::{
    PaymentIntent, PaymentIntentCancellationReason, PaymentIntentCaptureMethod, PaymentIntentStatus,
};

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

/// Where a payment stands with the provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaymentState {
    /// Waiting for the shopper: the card form, or a bank's challenge.
    Open,
    /// The shopper confirmed it and the provider is settling it (a payment
    /// method that takes days).
    Processing,
    /// The shopper confirmed it: the money is held on their card, to be
    /// taken or given back.
    Authorized,
    /// The money was taken.
    Captured,
    /// Cancelled: the hold given back, or lapsed, or the form closed.
    Cancelled,
}

/// A payment as the provider sees it now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaymentStatus {
    pub payment_id: String,
    pub state: PaymentState,
    /// What the provider holds and may take, in minor units of `currency`.
    pub amount_capturable: u64,
    /// What was taken, in minor units of `currency`.
    pub amount_received: u64,
    /// ISO-4217, lowercase as Stripe writes it.
    pub currency: String,
    /// Unix seconds the hold lapses at, for a card.
    pub capture_before: Option<u64>,
    /// Cancelled by the provider on its own: the hold lapsed.
    pub lapsed: bool,
    pub metadata: HashMap<String, String>,
}

/// What came of asking the provider to cancel a payment the shopper has not
/// confirmed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cancellation {
    /// The payment is cancelled (now, or it already was): the shopper's
    /// card form cannot confirm it any more, and nothing was charged.
    Cancelled,
    /// The shopper confirmed it and the provider is settling it (a payment
    /// method that takes days, a bank debit): it cannot be cancelled any
    /// more, and the webhook settles it. A card form still open, a bank's
    /// challenge included, is not this: it is cancelled.
    Confirming,
    /// The shopper confirmed it and the money is held on their card: the
    /// webhook turns it into the order, or releases it.
    Authorized,
    /// The money was taken: the webhook settles it.
    Paid,
}

/// What came of asking the provider to take the held money.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CaptureAnswer {
    /// Taken (now, or it already was).
    Captured { amount_cents: u64 },
    /// The hold is gone, nothing taken: it lapsed (`lapsed`), or was
    /// cancelled in the provider's own dashboard.
    Cancelled { lapsed: bool },
    /// The provider would not take it; what it said.
    Refused(String),
}

/// What came of asking the provider to give a hold back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReleaseAnswer {
    /// Given back (now, or it already was): nothing taken.
    Released,
    /// The money was taken in between (in the provider's own dashboard):
    /// it is refunded instead.
    Captured { amount_cents: u64 },
}

/// A request the provider refused (a 4xx answer other than 409 and 429):
/// asking again the same way gets the same answer. Any other error (no
/// answer, a 5xx, a 429, a [`Busy`]) is worth asking again later.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refused(pub String);

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Refused {}

/// A request the provider could not take at that moment, another one being
/// under way on the same thing (a 409: a request with the same idempotency
/// key not answered yet, or the object locked by another): asked again in a
/// moment, it is answered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Busy(pub String);

impl std::fmt::Display for Busy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Busy {}

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
    /// Takes the money held for the payment. An error is a provider that
    /// could not be reached: asked again later, it answers.
    async fn capture_payment(
        &self,
        secret_key: &str,
        payment_id: &str,
    ) -> anyhow::Result<CaptureAnswer>;
    /// Gives the hold of the payment back, nothing taken. An error is a
    /// provider that could not be reached, or that refused for a reason it
    /// gave.
    async fn release_payment(
        &self,
        secret_key: &str,
        payment_id: &str,
    ) -> anyhow::Result<ReleaseAnswer>;
}

/// Reads [`Cancellation`] off a payment's status, for one the shopper may
/// confirm no more; `None` while it may still be cancelled. Stripe cancels
/// a payment awaiting its method, its confirmation or the bank's challenge
/// (`requires_action`), so a shopper who left the challenge open is closed
/// like one who left the form open: the challenge does not expire by
/// itself, and finishing it days later would charge for goods long gone. A
/// payment Stripe is settling (`processing`) stays, and so does one the
/// shopper confirmed (`requires_capture`): its money is held for an order.
fn settled(status: &PaymentIntentStatus) -> Option<Cancellation> {
    match status {
        PaymentIntentStatus::Canceled => Some(Cancellation::Cancelled),
        PaymentIntentStatus::Succeeded => Some(Cancellation::Paid),
        PaymentIntentStatus::Processing => Some(Cancellation::Confirming),
        PaymentIntentStatus::RequiresCapture => Some(Cancellation::Authorized),
        // Cancellable, or a status this crate does not know, tried as well:
        // Stripe refuses the cancel itself when it must.
        _ => None,
    }
}

/// Where a payment stands, read off its status.
fn state_of(status: &PaymentIntentStatus) -> PaymentState {
    match status {
        PaymentIntentStatus::RequiresCapture => PaymentState::Authorized,
        PaymentIntentStatus::Succeeded => PaymentState::Captured,
        PaymentIntentStatus::Canceled => PaymentState::Cancelled,
        PaymentIntentStatus::Processing => PaymentState::Processing,
        _ => PaymentState::Open,
    }
}

/// The payment as [`PaymentStatus`] says it.
fn status_of(intent: PaymentIntent) -> PaymentStatus {
    let capture_before = intent
        .latest_charge
        .as_ref()
        .and_then(|charge| charge.as_object())
        .and_then(|charge| charge.payment_method_details.as_ref())
        .and_then(|details| details.card.as_ref())
        .and_then(|card| card.capture_before)
        .map(|at| at.max(0) as u64);
    PaymentStatus {
        payment_id: intent.id.to_string(),
        state: state_of(&intent.status),
        amount_capturable: intent.amount_capturable.max(0) as u64,
        amount_received: intent.amount_received.max(0) as u64,
        currency: intent.currency.to_string(),
        capture_before,
        lapsed: matches!(
            intent.cancellation_reason,
            Some(
                PaymentIntentCancellationReason::Automatic
                    | PaymentIntentCancellationReason::Expired
            )
        ),
        metadata: intent.metadata,
    }
}

/// Whether an error is a refusal the provider gave (as opposed to no
/// answer).
fn refusal(err: &anyhow::Error) -> Option<&Refused> {
    err.downcast_ref::<Refused>()
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
        let value = request.send(client).await.map_err(|err| match err {
            StripeError::Stripe(errors, 409) => anyhow::Error::new(Busy(
                errors
                    .message
                    .unwrap_or_else(|| "Stripe is busy with another request (409)".to_owned()),
            )),
            StripeError::Stripe(errors, status)
                if (400..500).contains(&status) && status != 429 =>
            {
                anyhow::Error::new(Refused(
                    errors
                        .message
                        .unwrap_or_else(|| format!("Stripe refused the request ({status})")),
                ))
            }
            err => anyhow::anyhow!("{err}"),
        })?;
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
        // Held only: taken once the order may be filled, or given back.
        let mut request = CreatePaymentIntent::new(payment.amount_cents as i64, currency)
            .automatic_payment_methods(CreatePaymentIntentAutomaticPaymentMethods::new(true))
            .capture_method(PaymentIntentCaptureMethod::Manual)
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
        // The charge says until when the card holds the money.
        let request =
            RetrievePaymentIntent::new(payment_id).expand(vec!["latest_charge".to_owned()]);
        let intent = send(&request, None, &client).await?;
        Ok(status_of(intent))
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

    /// A refusal is read against the payment as it then stands: taken
    /// already (another node, the merchant in Stripe's dashboard) is taken,
    /// a hold that lapsed is gone; anything else is the provider's reason.
    async fn capture_payment(
        &self,
        secret_key: &str,
        payment_id: &str,
    ) -> anyhow::Result<CaptureAnswer> {
        let client = self.client(secret_key)?;
        let refused = match send(&CapturePaymentIntent::new(payment_id), None, &client).await {
            Ok(intent) => {
                return Ok(CaptureAnswer::Captured {
                    amount_cents: intent.amount_received.max(0) as u64,
                });
            }
            Err(err) => match refusal(&err) {
                Some(refused) => refused.clone(),
                None => return Err(err),
            },
        };
        let status = self.payment_status(secret_key, payment_id).await?;
        Ok(match status.state {
            PaymentState::Captured => CaptureAnswer::Captured {
                amount_cents: status.amount_received,
            },
            PaymentState::Cancelled => CaptureAnswer::Cancelled {
                lapsed: status.lapsed,
            },
            _ => CaptureAnswer::Refused(refused.0),
        })
    }

    /// Read first, then cancelled: a hold cancelled already is given back,
    /// money taken already is told.
    async fn release_payment(
        &self,
        secret_key: &str,
        payment_id: &str,
    ) -> anyhow::Result<ReleaseAnswer> {
        let client = self.client(secret_key)?;
        let answer = |status: PaymentStatus| match status.state {
            PaymentState::Cancelled => Some(ReleaseAnswer::Released),
            PaymentState::Captured => Some(ReleaseAnswer::Captured {
                amount_cents: status.amount_received,
            }),
            _ => None,
        };
        if let Some(settled) = answer(self.payment_status(secret_key, payment_id).await?) {
            return Ok(settled);
        }
        let cancel = CancelPaymentIntent::new(payment_id)
            .cancellation_reason(CancelPaymentIntentCancellationReason::RequestedByCustomer);
        let refused = match send(&cancel, None, &client).await {
            Ok(_) => return Ok(ReleaseAnswer::Released),
            Err(err) => err,
        };
        answer(self.payment_status(secret_key, payment_id).await?).ok_or(refused)
    }
}

#[cfg(test)]
mod tests {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use super::*;

    /// Stripe answering every request with `status` and the error `body`;
    /// its address.
    async fn stand_in(status: &'static str, body: &'static str) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                read_request(&mut socket).await;
                let answer = format!(
                    "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = socket.write_all(answer.as_bytes()).await;
            }
        });
        url
    }

    /// Reads a request through to the end of its body.
    async fn read_request(socket: &mut tokio::net::TcpStream) {
        let mut request = Vec::new();
        let mut chunk = [0; 4096];
        loop {
            if let Some(end) = request.windows(4).position(|w| w == b"\r\n\r\n") {
                let head = String::from_utf8_lossy(&request[..end]).to_ascii_lowercase();
                let length = head
                    .lines()
                    .find_map(|line| line.strip_prefix("content-length:"))
                    .and_then(|value| value.trim().parse::<usize>().ok())
                    .unwrap_or(0);
                if request.len() >= end + 4 + length {
                    return;
                }
            }
            match socket.read(&mut chunk).await {
                Ok(0) | Err(_) => return,
                Ok(read) => request.extend_from_slice(&chunk[..read]),
            }
        }
    }

    fn payment() -> NewPayment {
        NewPayment {
            amount_cents: 2500,
            currency: "EUR".into(),
            idempotency_key: "timada-request".into(),
            metadata: HashMap::new(),
            receipt_email: None,
            description: None,
        }
    }

    #[tokio::test]
    async fn stripe_busy_with_the_same_key_is_asked_again_not_refused() {
        let busy = StripeGateway::new(Some(
            stand_in(
                "409 Conflict",
                r#"{"error":{"type":"idempotency_error","code":"idempotency_key_in_use","message":"There is currently another in-progress request using this Stripe-Idempotency-Key"}}"#,
            )
            .await,
        ));
        let err = busy
            .open_payment("sk_test_123", payment())
            .await
            .unwrap_err();
        assert!(err.downcast_ref::<Busy>().is_some(), "{err:#}");
        assert!(refusal(&err).is_none(), "{err:#}");

        // Any other 4xx is still Stripe's refusal.
        let declined = StripeGateway::new(Some(
            stand_in(
                "402 Payment Required",
                r#"{"error":{"type":"card_error","code":"card_declined","message":"Your card was declined."}}"#,
            )
            .await,
        ));
        let err = declined
            .open_payment("sk_test_123", payment())
            .await
            .unwrap_err();
        assert_eq!(
            refusal(&err),
            Some(&Refused("Your card was declined.".into()))
        );

        // Too many requests: worth asking again later, but not busy.
        let limited = StripeGateway::new(Some(
            stand_in(
                "429 Too Many Requests",
                r#"{"error":{"type":"invalid_request_error","message":"Too many requests."}}"#,
            )
            .await,
        ));
        let err = limited
            .open_payment("sk_test_123", payment())
            .await
            .unwrap_err();
        assert!(refusal(&err).is_none(), "{err:#}");
        assert!(err.downcast_ref::<Busy>().is_none(), "{err:#}");
    }

    #[test]
    fn a_payment_is_cancelled_unless_confirmed() {
        for cancellable in [
            PaymentIntentStatus::RequiresPaymentMethod,
            PaymentIntentStatus::RequiresConfirmation,
            PaymentIntentStatus::RequiresAction,
        ] {
            assert_eq!(settled(&cancellable), None, "{cancellable:?}");
        }
        assert_eq!(
            settled(&PaymentIntentStatus::RequiresCapture),
            Some(Cancellation::Authorized),
            "its money is held for an order"
        );
        assert_eq!(
            settled(&PaymentIntentStatus::Processing),
            Some(Cancellation::Confirming)
        );
        assert_eq!(
            settled(&PaymentIntentStatus::Succeeded),
            Some(Cancellation::Paid)
        );
        assert_eq!(
            settled(&PaymentIntentStatus::Canceled),
            Some(Cancellation::Cancelled)
        );
    }
}
