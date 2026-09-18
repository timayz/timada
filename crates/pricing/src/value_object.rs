use bitcode::{Decode, Encode};
use timada_core::Money;

/// A "payez en Nx" offer: `count` equal installments plus a flat fee.
#[derive(Debug, Clone, PartialEq, Eq, Default, Encode, Decode)]
pub struct InstallmentOffer {
    pub count: u8,
    pub fee: Money,
}
