//! What a code would take off an order if it were placed now — the cart and
//! checkout pages show it; the binding answer is `Command::redeem_code`.

use evento::Executor;
use timada_core::Money;

use crate::{
    command::{discount_id, voucher_id},
    value_object::{CodeKind, CodeRedemption, normalize_code},
};

use super::{load_discount_details, load_voucher_balance};

/// `None` when the code is unknown, unusable right now, or worth nothing on
/// this order (including a currency mismatch).
pub async fn quote_code<E: Executor>(
    executor: &E,
    code: &str,
    subtotal: &Money,
    max: &Money,
) -> anyhow::Result<Option<CodeRedemption>> {
    let now = timada_core::time::now_unix_secs()?;
    let (kind, amount) = if let Some(discount) =
        load_discount_details(executor, discount_id(code)).await?
    {
        let exhausted = discount
            .max_redemptions
            .is_some_and(|cap| discount.redeemed >= cap);
        if !discount.active || exhausted || discount.valid_until.is_some_and(|until| until < now) {
            return Ok(None);
        }
        let Ok(amount) = discount.kind.amount_off(subtotal, max) else {
            return Ok(None);
        };
        (CodeKind::Discount, amount)
    } else if let Some(voucher) = load_voucher_balance(executor, voucher_id(code)).await? {
        if voucher.cancelled
            || voucher.expires_at.is_some_and(|at| at < now)
            || voucher.remaining.same_currency(max).is_err()
        {
            return Ok(None);
        }
        let amount = Money::new(voucher.remaining.minor.min(max.minor), &max.currency);
        (CodeKind::Voucher, amount)
    } else {
        return Ok(None);
    };

    Ok(amount.is_positive().then(|| CodeRedemption {
        code: normalize_code(code),
        kind,
        amount,
    }))
}

/// The currency a code is bound to: a voucher's, or a fixed-amount promo
/// code's. `None` for a percentage — it works in every currency — and for a
/// code nobody knows. What a page needs to say *why* a code does nothing on
/// a cart in another currency: a value is never converted.
pub async fn code_currency<E: Executor>(
    executor: &E,
    code: &str,
) -> anyhow::Result<Option<String>> {
    if let Some(discount) = load_discount_details(executor, discount_id(code)).await? {
        return Ok(match discount.kind {
            crate::value_object::DiscountKind::FixedAmount { amount } => Some(amount.currency),
            crate::value_object::DiscountKind::Percent { .. } => None,
        });
    }
    Ok(load_voucher_balance(executor, voucher_id(code))
        .await?
        .map(|voucher| voucher.remaining.currency))
}
