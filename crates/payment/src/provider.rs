//! The payment service provider, as the payment context sees it: something
//! that lets a shopper pay a requested payment, can call a payment session
//! off, and gives money back. The host picks the implementation;
//! [`ManualProvider`] — an operator captures by hand, nothing is sent anywhere —
//! is what a host without a provider runs on.

use std::{
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex},
};

use timada_core::Money;

use crate::{query::PaymentView, value_object::PaymentMethod};

/// The result of a provider call, boxed so providers can be `dyn`.
pub type ProviderFuture<'a, T> =
    Pin<Box<dyn Future<Output = Result<T, ProviderError>> + Send + 'a>>;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ProviderError {
    /// The provider said no and will keep saying no: retrying is pointless.
    #[error("refused: {0}")]
    Refused(String),
    /// The provider could not be reached or failed on its side: try later.
    #[error("unavailable: {0}")]
    Unavailable(String),
}

/// Where the provider sends the shopper back to once they paid (or gave up).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReturnUrls {
    pub paid: String,
}

/// What the storefront must do for the shopper to pay.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PaymentStart {
    /// Nothing: an operator captures the payment by hand.
    Manual,
    /// Render the provider's embedded form with these.
    ClientSecret {
        client_secret: String,
        publishable_key: String,
    },
    /// Send the shopper to the provider's own page.
    Redirect(String),
}

/// A payment session as opened by [`PaymentProvider::start`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartedPayment {
    /// The provider's id for the session (a payment intent, a checkout
    /// session…), kept so the same session is reused and can be cancelled.
    pub session_reference: Option<String>,
    pub start: PaymentStart,
}

/// What became of a session the shop wanted to call off.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CancelOutcome {
    /// Nobody can pay on it any more.
    Cancelled,
    /// Too late: the shopper paid. `reference` is what a capture records.
    AlreadyPaid { reference: String },
}

/// One refund handed to the provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderRefund {
    /// The `psp_reference` the capture recorded.
    pub psp_reference: String,
    pub amount: Money,
    /// The same key never refunds twice, however often it is sent.
    pub idempotency_key: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RefundOutcome {
    /// The money is on its way back; `reference` is the provider's refund id.
    Settled { reference: String },
    /// The provider took the refund and will tell later (a
    /// [`crate::ProviderEvent`]) what became of it.
    Pending { reference: String },
}

pub trait PaymentProvider: Send + Sync {
    /// Whether the shopper can pay this way; the storefront only offers the
    /// methods its provider supports.
    fn supports(&self, method: &PaymentMethod) -> bool;

    /// Opens — or, given the `session` of an earlier call, reopens — what the
    /// shopper pays on. Must be idempotent per payment.
    fn start<'a>(
        &'a self,
        payment: &'a PaymentView,
        session: Option<&'a str>,
        urls: &'a ReturnUrls,
    ) -> ProviderFuture<'a, StartedPayment>;

    /// Calls a session off, when the payment timed out.
    fn cancel<'a>(&'a self, session: &'a str) -> ProviderFuture<'a, CancelOutcome>;

    fn refund<'a>(&'a self, refund: &'a ProviderRefund) -> ProviderFuture<'a, RefundOutcome>;
}

/// No provider: an operator captures payments by hand (the admin's button) and
/// gives money back by their own means, so a refund settles at once.
#[derive(Debug, Clone, Copy, Default)]
pub struct ManualProvider;

impl PaymentProvider for ManualProvider {
    fn supports(&self, _method: &PaymentMethod) -> bool {
        true
    }

    fn start<'a>(
        &'a self,
        _payment: &'a PaymentView,
        _session: Option<&'a str>,
        _urls: &'a ReturnUrls,
    ) -> ProviderFuture<'a, StartedPayment> {
        Box::pin(async {
            Ok(StartedPayment {
                session_reference: None,
                start: PaymentStart::Manual,
            })
        })
    }

    fn cancel<'a>(&'a self, _session: &'a str) -> ProviderFuture<'a, CancelOutcome> {
        Box::pin(async { Ok(CancelOutcome::Cancelled) })
    }

    fn refund<'a>(&'a self, refund: &'a ProviderRefund) -> ProviderFuture<'a, RefundOutcome> {
        Box::pin(async move {
            Ok(RefundOutcome::Settled {
                reference: format!("manual-refund-{}", refund.idempotency_key),
            })
        })
    }
}

/// A provider for tests: it opens redirect sessions, records what it is asked
/// and answers refunds and cancellations as scripted.
#[derive(Debug, Clone, Default)]
pub struct FakeProvider {
    inner: Arc<Mutex<FakeState>>,
}

#[derive(Debug, Default)]
struct FakeState {
    refunds: Vec<ProviderRefund>,
    cancelled: Vec<String>,
    refund_answers: Vec<Result<RefundOutcome, ProviderError>>,
    paid_sessions: Vec<String>,
    card_only: bool,
    embedded: bool,
}

impl FakeProvider {
    fn state(&self) -> std::sync::MutexGuard<'_, FakeState> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// A provider that only takes cards.
    pub fn card_only() -> Self {
        let provider = Self::default();
        provider.state().card_only = true;
        provider
    }

    /// A card-only provider paid on an embedded form rather than on a page of
    /// its own: [`PaymentStart::ClientSecret`], the secret being
    /// `"{session}_secret"` and the key `pk_fake`.
    pub fn embedded() -> Self {
        let provider = Self::card_only();
        provider.state().embedded = true;
        provider
    }

    /// Queues the answer to the next refund; unscripted refunds settle.
    pub fn answer_refund(&self, answer: Result<RefundOutcome, ProviderError>) {
        self.state().refund_answers.push(answer);
    }

    /// The shopper paid on that session: cancelling it comes too late.
    pub fn mark_paid(&self, session: &str) {
        self.state().paid_sessions.push(session.to_owned());
    }

    /// Every refund the provider was asked for, retries included.
    pub fn refunds(&self) -> Vec<ProviderRefund> {
        self.state().refunds.clone()
    }

    pub fn cancelled(&self) -> Vec<String> {
        self.state().cancelled.clone()
    }

    /// The session [`Self::start`] opens for a payment.
    pub fn session_of(payment_id: &str) -> String {
        format!("fake-session-{payment_id}")
    }
}

impl PaymentProvider for FakeProvider {
    fn supports(&self, method: &PaymentMethod) -> bool {
        !self.state().card_only || *method == PaymentMethod::Card
    }

    fn start<'a>(
        &'a self,
        payment: &'a PaymentView,
        session: Option<&'a str>,
        urls: &'a ReturnUrls,
    ) -> ProviderFuture<'a, StartedPayment> {
        Box::pin(async move {
            let session = session.map_or_else(|| Self::session_of(&payment.id), str::to_owned);
            let start = if self.state().embedded {
                PaymentStart::ClientSecret {
                    client_secret: format!("{session}_secret"),
                    publishable_key: "pk_fake".to_owned(),
                }
            } else {
                PaymentStart::Redirect(format!("https://pay.invalid/{session}?back={}", urls.paid))
            };
            Ok(StartedPayment {
                start,
                session_reference: Some(session),
            })
        })
    }

    fn cancel<'a>(&'a self, session: &'a str) -> ProviderFuture<'a, CancelOutcome> {
        Box::pin(async move {
            let mut state = self.state();
            if state.paid_sessions.iter().any(|s| s == session) {
                return Ok(CancelOutcome::AlreadyPaid {
                    reference: format!("fake-paid-{session}"),
                });
            }
            state.cancelled.push(session.to_owned());
            Ok(CancelOutcome::Cancelled)
        })
    }

    fn refund<'a>(&'a self, refund: &'a ProviderRefund) -> ProviderFuture<'a, RefundOutcome> {
        Box::pin(async move {
            let mut state = self.state();
            state.refunds.push(refund.clone());
            if state.refund_answers.is_empty() {
                return Ok(RefundOutcome::Settled {
                    reference: format!("fake-refund-{}", refund.idempotency_key),
                });
            }
            state.refund_answers.remove(0)
        })
    }
}
