use bitcode::{Decode, Encode};
use timada_core::Money;

#[derive(Debug, Clone, PartialEq, Eq, Encode, Decode)]
pub enum DiscountKind {
    /// Percentage off in basis points (1000 = 10 %).
    Percent {
        bp: u16,
    },
    FixedAmount {
        amount: Money,
    },
}

impl Default for DiscountKind {
    fn default() -> Self {
        DiscountKind::Percent { bp: 0 }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Encode, Decode)]
pub enum VoucherKind {
    #[default]
    GiftVoucher,
    /// "Avoir": credit issued for a returned or cancelled order.
    CreditNote { origin_order_id: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Encode, Decode)]
pub struct VoucherRedemption {
    pub order_id: String,
    pub amount: Money,
}

/// Codes are stored and matched upper-cased and trimmed.
pub fn normalize_code(code: &str) -> String {
    code.trim().to_uppercase()
}
