use bitcode::{Decode, Encode};
use timada_core::{Money, MoneyError};

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

impl DiscountKind {
    /// What the code takes off `subtotal`, never more than `max` (the part of
    /// the order a reduction may cover). `Err` on a currency mismatch.
    pub fn amount_off(&self, subtotal: &Money, max: &Money) -> Result<Money, MoneyError> {
        subtotal.same_currency(max)?;
        let amount = match self {
            DiscountKind::Percent { bp } => subtotal.percent_bp(*bp),
            DiscountKind::FixedAmount { amount } => {
                amount.same_currency(subtotal)?;
                amount.clone()
            }
        };
        Ok(Money::new(
            amount.minor.min(max.minor).max(0),
            &subtotal.currency,
        ))
    }
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

/// Which kind of code the "code promo ou bon d'achat" box turned out to hold.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodeKind {
    Discount,
    Voucher,
}

/// What a code is worth on one order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodeRedemption {
    pub code: String,
    pub kind: CodeKind,
    pub amount: Money,
}

/// Codes are stored and matched upper-cased and trimmed.
pub fn normalize_code(code: &str) -> String {
    code.trim().to_uppercase()
}
