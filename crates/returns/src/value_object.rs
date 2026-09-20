use bitcode::{Decode, Encode};
use timada_core::{Money, MoneyError};
use timada_order::{OrderDetailsView, PromoKind};

/// A line of the order the customer wants to send back, frozen with the name
/// and the price it was bought at.
#[derive(Debug, Clone, PartialEq, Eq, Default, Encode, Decode)]
pub struct ReturnLine {
    pub product_id: String,
    pub name: String,
    pub quantity: u32,
    pub unit_price: Money,
}

/// What the operator found in the parcel for one requested line.
#[derive(Debug, Clone, PartialEq, Eq, Default, Encode, Decode)]
pub struct ReceivedLine {
    pub product_id: String,
    /// Units taken back; fewer than requested when some are missing or refused.
    pub accepted: u32,
    /// Whether the accepted units are fit to be sold again.
    pub restock: bool,
}

/// Units sent again for the ones a return brought back.
#[derive(Debug, Clone, PartialEq, Eq, Default, Encode, Decode)]
pub struct ReplacementLine {
    pub product_id: String,
    pub name: String,
    pub quantity: u32,
}

/// Where the replacement of a return stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Encode, Decode)]
pub enum ReplacementStatus {
    /// Decided when the parcel was received; stock not reserved yet.
    #[default]
    Planned,
    /// A parcel was created for it.
    Arranged,
    /// Impossible after all: the return was refunded.
    Abandoned,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Encode, Decode)]
pub enum RefundMethod {
    /// Back to the means of payment, as the legal withdrawal right requires.
    #[default]
    OriginalPayment,
    /// A store-credit voucher ("avoir") instead of money.
    StoreCredit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Encode, Decode)]
pub enum ReturnStatus {
    #[default]
    Requested,
    Approved,
    Refused,
    Cancelled,
    /// Received; the process manager is restocking and refunding.
    Received,
    Completed,
}

impl ReturnStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            ReturnStatus::Requested => "requested",
            ReturnStatus::Approved => "approved",
            ReturnStatus::Refused => "refused",
            ReturnStatus::Cancelled => "cancelled",
            ReturnStatus::Received => "received",
            ReturnStatus::Completed => "completed",
        }
    }

    /// Still holding the units it claims against the order.
    pub fn is_open(self) -> bool {
        matches!(self, ReturnStatus::Requested | ReturnStatus::Approved)
    }
}

/// How long after shipping a return may be asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReturnPolicy {
    pub window_days: u32,
}

impl Default for ReturnPolicy {
    /// The French legal withdrawal period ("droit de rétractation").
    fn default() -> Self {
        Self { window_days: 14 }
    }
}

impl ReturnPolicy {
    /// The last second (Unix) a return of an order shipped at `shipped_at`
    /// may be requested.
    pub fn deadline(&self, shipped_at: u64) -> u64 {
        shipped_at.saturating_add(u64::from(self.window_days) * 86_400)
    }
}

/// What goes back to the customer for the accepted units.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefundSplit {
    /// To the original payment.
    pub money: Money,
    /// As a store-credit voucher.
    pub credit: Money,
}

/// Values the accepted units at the price paid. The order's discount is
/// spread over the goods pro rata: a promo code was a price reduction, so its
/// share is simply not refunded; a voucher was a means of payment, so its
/// share comes back as store credit. Shipping and handling fees stay paid.
/// With [`RefundMethod::StoreCredit`] everything comes back as credit.
pub fn refund_split(
    order: &OrderDetailsView,
    accepted: &[(&Money, u32)],
    method: RefundMethod,
) -> Result<RefundSplit, MoneyError> {
    let currency = order.total.currency.clone();
    let mut gross = Money::zero(&currency);
    for (unit_price, quantity) in accepted {
        gross = gross.checked_add(&unit_price.checked_mul(*quantity)?)?;
    }

    let (discount_minor, by_voucher) = match &order.discount {
        Some(discount) if order.subtotal.minor > 0 => {
            let share = i128::from(discount.amount.minor) * i128::from(gross.minor)
                / i128::from(order.subtotal.minor);
            (share as i64, discount.kind == PromoKind::Voucher)
        }
        _ => (0, false),
    };
    let discount_share = Money::new(discount_minor.clamp(0, gross.minor), &currency);
    let paid = gross.checked_sub(&discount_share)?;
    let given_back_as_credit = if by_voucher {
        discount_share
    } else {
        Money::zero(&currency)
    };

    Ok(match method {
        RefundMethod::OriginalPayment => RefundSplit {
            money: paid,
            credit: given_back_as_credit,
        },
        RefundMethod::StoreCredit => RefundSplit {
            money: Money::zero(&currency),
            credit: paid.checked_add(&given_back_as_credit)?,
        },
    })
}
