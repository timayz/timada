//! Pure validity and amount rules, shared by the advisory cart check and the
//! authoritative checkout re-check.

use timada_core::Money;

use crate::aggregate::DiscountKind;
use crate::view::DiscountView;

/// Why a code cannot be used, in words a customer can act on.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum DiscountRefusal {
    #[error("this code does not exist")]
    UnknownCode,
    #[error("this code is no longer active")]
    Disabled,
    #[error("this code is not valid yet")]
    NotStarted,
    #[error("this code has expired")]
    Expired,
    #[error("this code has been fully used")]
    FullyRedeemed,
    #[error("this code applies to a different currency than your cart")]
    CurrencyMismatch,
}

/// Is the code usable right now? Usage-limit exhaustion is not checked here —
/// only the atomic [`redeem`](crate::redeem) can answer that without racing.
pub fn validate(discount: &DiscountView, now_millis: i64) -> Result<(), DiscountRefusal> {
    if discount.disabled {
        return Err(DiscountRefusal::Disabled);
    }
    if now_millis < discount.starts_at {
        return Err(DiscountRefusal::NotStarted);
    }
    if let Some(ends_at) = discount.ends_at
        && now_millis > ends_at
    {
        return Err(DiscountRefusal::Expired);
    }
    Ok(())
}

/// How much comes off a tax-inclusive `gross_total`.
///
/// Percentages truncate — the fractional cent stays with the store; fixed
/// amounts must match the cart's currency and clamp to the total so it never
/// goes negative.
pub fn discount_amount(kind: DiscountKind, gross_total: Money) -> Result<Money, DiscountRefusal> {
    match kind {
        DiscountKind::Percentage { bps } => {
            let cents = (i128::from(gross_total.amount_cents) * i128::from(bps) / 10_000) as i64;
            Ok(Money::new(cents, gross_total.currency))
        }
        DiscountKind::Fixed { amount } => {
            if amount.currency != gross_total.currency {
                return Err(DiscountRefusal::CurrencyMismatch);
            }
            Ok(Money::new(
                amount.amount_cents.min(gross_total.amount_cents),
                gross_total.currency,
            ))
        }
    }
}
