//! Checkout: the one write-side command a customer can issue on an order.
//!
//! Everything after this point is the saga's job — see [`crate::saga`].

use std::collections::HashMap;
use std::sync::Arc;

use sqlx::SqlitePool;
use timada_cart::{CartLine, load_cart, mark_checked_out};
use timada_core::{Executor, Money};
use timada_promotion::{discount_amount, discount_id, load_discount, redeem, validate};
use timada_tax::{TaxAssessmentRequest, TaxCalculator, TaxError, TaxableLine, TaxedLine};

use crate::aggregate::{Address, OrderLine, OrderPlaced};

/// Why a checkout was refused.
///
/// Split from the storage failures so the storefront answers a bad request
/// with a 400 and a real reason instead of a blanket 500.
#[derive(Debug, thiserror::Error)]
pub enum PlaceOrderError {
    #[error("this cart does not exist")]
    UnknownCart,
    #[error("cannot check out an empty cart")]
    EmptyCart,
    #[error("this cart has already been checked out")]
    CartAlreadyCheckedOut,
    #[error("{0}")]
    Invalid(String),
    #[error(transparent)]
    Storage(#[from] anyhow::Error),
}

/// Turn a cart into an order.
///
/// The lines, the total and the tax breakdown are snapshotted onto
/// `OrderPlaced`: from here on the order quotes what the customer agreed to,
/// whatever the catalog or the tax rates do next.
///
/// Tax is assessed exactly once, here, and only after the address is known —
/// the rate depends on the destination country. Prices are tax-inclusive, so
/// the total the customer saw is the total that gets charged; the assessment
/// only says how much of it is tax.
///
/// **Two aggregates, two commits.** The order is created first, then the cart
/// is marked checked out. A crash in between leaves a placed order and a cart
/// the customer could check out a second time — the cheaper failure of the two,
/// since the alternative (closing the cart first) can lose an order the
/// customer already believes they placed. A duplicate order is visible in the
/// admin list and refundable; a lost one is not recoverable at all.
///
/// The same shape of reasoning fixes where the discount is redeemed:
/// `redeem_pool` (the single-connection write pool) decrements the atomic
/// usage counter *before* the `OrderPlaced` commit, so a crash in between
/// leaks one redemption slot — cheaper than the over-redemption the other
/// order would allow.
#[tracing::instrument(skip(executor, tax, redeem_pool, shipping_address))]
pub async fn place_order(
    executor: &Executor,
    tax: &Arc<dyn TaxCalculator>,
    redeem_pool: &SqlitePool,
    cart_id: &str,
    customer_id: Option<String>,
    email: String,
    shipping_address: Address,
) -> Result<String, PlaceOrderError> {
    let Some(cart) = load_cart(executor, cart_id).await? else {
        return Err(PlaceOrderError::UnknownCart);
    };
    if cart.checked_out {
        return Err(PlaceOrderError::CartAlreadyCheckedOut);
    }
    if cart.is_empty() {
        return Err(PlaceOrderError::EmptyCart);
    }

    let email = email.trim().to_owned();
    if !email.contains('@') {
        return Err(PlaceOrderError::Invalid(
            "a valid email address is required".to_owned(),
        ));
    }
    let shipping_address = validate_address(shipping_address)?;

    // The authoritative discount check — the cart's advisory one may be
    // arbitrarily stale by now. The amount is derived against the current
    // total and split across lines so the tax on each line is extracted from
    // what the customer actually pays for it.
    let discount = match cart.discount_code.as_deref() {
        Some(code) => {
            let view = load_discount(executor, &discount_id(code))
                .await?
                .ok_or_else(|| {
                    PlaceOrderError::Invalid(format!("discount {code}: this code does not exist"))
                })?;
            validate(&view, timada_core::now_millis()).map_err(|refused| {
                PlaceOrderError::Invalid(format!("discount {code}: {refused}"))
            })?;
            let amount = discount_amount(view.kind, cart.total()).map_err(|refused| {
                PlaceOrderError::Invalid(format!("discount {code}: {refused}"))
            })?;
            Some((view, amount))
        }
        None => None,
    };
    let per_line_discount = match &discount {
        Some((_, amount)) => allocate_discount(&cart.lines, *amount),
        None => vec![0; cart.lines.len()],
    };

    let assessment = tax
        .assess(TaxAssessmentRequest {
            country: shipping_address.country.clone(),
            lines: cart
                .lines
                .iter()
                .zip(&per_line_discount)
                .map(|(line, discount_cents)| TaxableLine {
                    reference: line.product_id.clone(),
                    gross_unit_price: line.unit_price,
                    quantity: line.quantity,
                    discount: Money::new(*discount_cents, line.unit_price.currency),
                })
                .collect(),
        })
        .await
        .map_err(|error| match error {
            // A country nobody can assess, or a cart the calculator refuses,
            // is something the customer can act on. Everything else is ours.
            error @ TaxError::Api(_) => PlaceOrderError::Storage(error.into()),
            refused => PlaceOrderError::Invalid(refused.to_string()),
        })?;

    // Prices are tax-inclusive, so a calculator that changed the total has
    // misunderstood the contract — charging anything but what the customer
    // saw (minus the discount they applied) is not something to paper over.
    let expected_total = match &discount {
        Some((_, amount)) => cart
            .total()
            .subtract(*amount)
            .map_err(anyhow::Error::from)?,
        None => cart.total(),
    };
    if assessment.total_gross != expected_total {
        return Err(PlaceOrderError::Storage(anyhow::anyhow!(
            "tax calculator `{}` returned gross {} for a checkout totalling {}",
            tax.id(),
            assessment.total_gross,
            expected_total
        )));
    }

    let taxed: HashMap<&str, &TaxedLine> = assessment
        .lines
        .iter()
        .map(|line| (line.reference.as_str(), line))
        .collect();

    let mut lines = Vec::with_capacity(cart.lines.len());
    for (line, discount_cents) in cart.lines.iter().zip(&per_line_discount) {
        let Some(taxed) = taxed.get(line.product_id.as_str()) else {
            return Err(PlaceOrderError::Storage(anyhow::anyhow!(
                "tax calculator `{}` returned no line for product {}",
                tax.id(),
                line.product_id
            )));
        };
        lines.push(order_line(line, *discount_cents, taxed));
    }

    // Consume the redemption slot last, once nothing else can refuse: only
    // the commit below can now fail, and that failure burns one slot instead
    // of a customer's money.
    let (discount_code, discount_amount) = match &discount {
        Some((view, amount)) => {
            if !redeem(redeem_pool, view).await? {
                return Err(PlaceOrderError::Invalid(format!(
                    "discount {}: this code has been fully used",
                    view.code
                )));
            }
            (Some(view.code.clone()), Some(*amount))
        }
        None => (None, None),
    };

    let order_id = evento::create()
        .event(&OrderPlaced {
            cart_id: cart_id.to_owned(),
            customer_id,
            email,
            shipping_address,
            lines,
            discount_code,
            discount_amount,
            total: assessment.total_gross,
            total_net: assessment.total_net,
            total_tax: assessment.total_tax,
        })
        .commit(executor)
        .await
        .map_err(anyhow::Error::from)?;

    tracing::info!(
        %order_id,
        cart_id,
        lines = cart.lines.len(),
        tax = %assessment.total_tax,
        "order placed"
    );

    mark_checked_out(executor, cart_id).await?;

    Ok(order_id)
}

/// Split `amount` across the lines, proportional to each line's gross, by
/// largest remainder: floor every share, then hand the leftover cents to the
/// lines with the biggest fractional remainders (earlier line wins a tie).
/// The shares always sum to exactly `amount`, and no share exceeds its line.
pub fn allocate_discount(lines: &[CartLine], amount: Money) -> Vec<i64> {
    let total: i64 = lines
        .iter()
        .map(|line| line.line_total().amount_cents)
        .sum();
    if total <= 0 || amount.amount_cents <= 0 {
        return vec![0; lines.len()];
    }
    // discount_amount clamps to the total, but allocate defensively anyway.
    let amount = amount.amount_cents.min(total);

    let mut shares = Vec::with_capacity(lines.len());
    let mut remainders = Vec::with_capacity(lines.len());
    let mut allocated = 0i64;
    for (index, line) in lines.iter().enumerate() {
        let numerator = i128::from(amount) * i128::from(line.line_total().amount_cents);
        let share = (numerator / i128::from(total)) as i64;
        let remainder = numerator % i128::from(total);
        allocated += share;
        shares.push(share);
        remainders.push((index, remainder));
    }

    let mut leftover = amount - allocated;
    remainders.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    for (index, _) in remainders {
        if leftover == 0 {
            break;
        }
        shares[index] += 1;
        leftover -= 1;
    }

    shares
}

/// Freeze a cart line, its discount share and its assessed tax into an order
/// line.
///
/// The gross amounts come from the cart — what the customer was shown — and
/// only the split comes from the calculator, so a calculator that rounds
/// oddly can never change the price.
fn order_line(line: &CartLine, discount_cents: i64, taxed: &TaxedLine) -> OrderLine {
    OrderLine {
        product_id: line.product_id.clone(),
        title: line.title.clone(),
        unit_price: line.unit_price,
        supplier_id: line.supplier_id.clone(),
        supplier_product_ref: line.supplier_product_ref.clone(),
        quantity: line.quantity,
        tax_rate_bps: taxed.tax_rate_bps,
        discount: Money::new(discount_cents, line.unit_price.currency),
        net: taxed.net,
        tax: taxed.tax,
    }
}

/// Refuse blank fields and nothing else.
///
/// Address formats are country-specific; a framework that rejects a valid
/// Japanese address for not looking French is worse than one that ships a typo.
fn validate_address(address: Address) -> Result<Address, PlaceOrderError> {
    let address = Address {
        full_name: address.full_name.trim().to_owned(),
        street: address.street.trim().to_owned(),
        city: address.city.trim().to_owned(),
        postal_code: address.postal_code.trim().to_owned(),
        country: address.country.trim().to_owned(),
    };

    let missing = [
        ("full name", &address.full_name),
        ("street", &address.street),
        ("city", &address.city),
        ("postal code", &address.postal_code),
        ("country", &address.country),
    ]
    .into_iter()
    .find(|(_, value)| value.is_empty());

    match missing {
        Some((field, _)) => Err(PlaceOrderError::Invalid(format!("{field} is required"))),
        None => Ok(address),
    }
}
