//! Checkout: the one write-side command a customer can issue on an order.
//!
//! Everything after this point is the saga's job — see [`crate::saga`].

use timada_cart::{load_cart, mark_checked_out};
use timada_core::Executor;

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
/// The lines and the total are snapshotted onto `OrderPlaced`: from here on the
/// order quotes what the customer agreed to, whatever the catalog does next.
///
/// **Two aggregates, two commits.** The order is created first, then the cart
/// is marked checked out. A crash in between leaves a placed order and a cart
/// the customer could check out a second time — the cheaper failure of the two,
/// since the alternative (closing the cart first) can lose an order the
/// customer already believes they placed. A duplicate order is visible in the
/// admin list and refundable; a lost one is not recoverable at all.
#[tracing::instrument(skip(executor, shipping_address))]
pub async fn place_order(
    executor: &Executor,
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

    let order_id = evento::create()
        .event(&OrderPlaced {
            cart_id: cart_id.to_owned(),
            email,
            shipping_address,
            lines: cart.lines.iter().map(OrderLine::from).collect(),
            total: cart.total(),
        })
        .commit(executor)
        .await
        .map_err(anyhow::Error::from)?;

    tracing::info!(%order_id, cart_id, lines = cart.lines.len(), "order placed");

    mark_checked_out(executor, cart_id).await?;

    Ok(order_id)
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
