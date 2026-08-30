//! The payment gateway port.
//!
//! Everything the payment context needs from the outside world sits behind
//! [`PaymentProvider`]. The domain never names a concrete gateway; it only
//! knows that a charge was captured or declined.

use timada_core::Money;

/// A charge to attempt against a provider.
#[derive(Debug, Clone)]
pub struct ChargeRequest {
    pub order_id: String,
    pub amount: Money,
}

/// What the provider decided about a [`ChargeRequest`].
///
/// A declined charge is a normal business outcome, not an error — only
/// transport/API trouble surfaces as [`PaymentError`].
#[derive(Debug, Clone)]
pub enum ChargeOutcome {
    Captured { provider_charge_ref: String },
    Declined { reason: String },
}

/// Something went wrong talking to the provider.
#[derive(Debug, thiserror::Error)]
pub enum PaymentError {
    #[error("payment provider does not implement this operation")]
    NotImplemented,
    #[error("payment provider api error: {0}")]
    Api(String),
}

#[async_trait::async_trait]
pub trait PaymentProvider: Send + Sync {
    /// Stable provider identifier, recorded on every `ChargeRequested` event.
    fn id(&self) -> &'static str;

    async fn charge(&self, req: ChargeRequest) -> Result<ChargeOutcome, PaymentError>;

    async fn refund(&self, provider_charge_ref: &str) -> Result<(), PaymentError>;
}
