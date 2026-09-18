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
