//! Checkout: the one write-side command a customer can issue on an order.
//!
//! Everything after this point is the saga's job — see [`crate::saga`].

use std::collections::HashMap;
use std::sync::Arc;

use timada_cart::{CartLine, load_cart, mark_checked_out};
use timada_core::Executor;
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
#[tracing::instrument(skip(executor, tax, shipping_address))]
pub async fn place_order(
    executor: &Executor,
    tax: &Arc<dyn TaxCalculator>,
    cart_id: &str,
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

    let assessment = tax
        .assess(TaxAssessmentRequest {
            country: shipping_address.country.clone(),
            lines: cart
                .lines
                .iter()
                .map(|line| TaxableLine {
                    reference: line.product_id.clone(),
                    gross_unit_price: line.unit_price,
                    quantity: line.quantity,
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
    // saw is not something to paper over.
    if assessment.total_gross != cart.total() {
        return Err(PlaceOrderError::Storage(anyhow::anyhow!(
            "tax calculator `{}` returned gross {} for a cart totalling {}",
            tax.id(),
            assessment.total_gross,
            cart.total()
        )));
    }

    let taxed: HashMap<&str, &TaxedLine> = assessment
        .lines
        .iter()
        .map(|line| (line.reference.as_str(), line))
        .collect();

    let mut lines = Vec::with_capacity(cart.lines.len());
    for line in &cart.lines {
        let Some(taxed) = taxed.get(line.product_id.as_str()) else {
            return Err(PlaceOrderError::Storage(anyhow::anyhow!(
                "tax calculator `{}` returned no line for product {}",
                tax.id(),
                line.product_id
            )));
        };
        lines.push(order_line(line, taxed));
    }

    let order_id = evento::create()
        .event(&OrderPlaced {
            cart_id: cart_id.to_owned(),
            email,
            shipping_address,
            lines,
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

/// Freeze a cart line and its assessed tax into an order line.
///
/// The gross amounts come from the cart — what the customer was shown — and
/// only the split comes from the calculator, so a calculator that rounds
/// oddly can never change the price.
fn order_line(line: &CartLine, taxed: &TaxedLine) -> OrderLine {
    OrderLine {
        product_id: line.product_id.clone(),
        title: line.title.clone(),
        unit_price: line.unit_price,
        supplier_id: line.supplier_id.clone(),
        supplier_product_ref: line.supplier_product_ref.clone(),
        quantity: line.quantity,
        tax_rate_bps: taxed.tax_rate_bps,
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
