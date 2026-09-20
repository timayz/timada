use bitcode::{Decode, Encode};
use timada_core::Money;

/// How the customer pays: in one go by card, or split into `count`
/// installments with a fixed fee ("Paiement en 3 fois").
#[derive(Debug, Clone, PartialEq, Eq, Default, Encode, Decode)]
pub enum PaymentMethod {
    #[default]
    Card,
    Installments {
        count: u8,
        fee: Money,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Encode, Decode)]
pub enum PaymentStatus {
    #[default]
    Requested,
    Captured,
    Declined,
    Refunded,
}

/// Where a refund stands between the shop's decision and the provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Encode, Decode)]
pub enum RefundStatus {
    /// Asked for; the provider has not confirmed yet.
    #[default]
    Pending,
    /// The money went back.
    Settled,
    /// The provider refused for good.
    Failed,
}

impl RefundStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Settled => "settled",
            Self::Failed => "failed",
        }
    }
}
