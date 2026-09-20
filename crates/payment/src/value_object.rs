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

/// Where a dispute stands with the cardholder's bank.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Encode, Decode)]
pub enum DisputeStatus {
    /// Waiting for the shop's evidence, or for the bank's decision.
    #[default]
    Open,
    Won,
    Lost,
}

impl DisputeStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Won => "won",
            Self::Lost => "lost",
        }
    }
}

/// A provider's dispute reason in the shop's language. Card networks share a
/// small vocabulary (these are Stripe's codes); anything else is shown as the
/// provider said it.
pub fn dispute_reason_label(reason: &str) -> &str {
    match reason {
        "fraudulent" => "paiement non reconnu par le titulaire de la carte",
        "product_not_received" => "produit non reçu",
        "product_unacceptable" => "produit non conforme",
        "duplicate" => "paiement en double",
        "credit_not_processed" => "remboursement attendu et non reçu",
        "subscription_canceled" => "abonnement résilié",
        "unrecognized" => "paiement non reconnu",
        "general" | "" => "motif non précisé",
        other => other,
    }
}
