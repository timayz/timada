mod add_line;
mod apply_promo_code;
mod change_line_quantity;
mod checkout;
mod open_cart;
mod remove_line;
mod remove_promo_code;
mod reprice_line;
mod save_cart;
mod saved_carts;

use std::ops::Deref;

pub use add_line::AddLine;
pub use checkout::Checkout;

use evento::{Executor, Projection, metadata::Event};

use crate::{
    aggregator::{
        Cart, CartAssignedToCustomer, CartCheckedOut, CartDiscarded, CartLineAdded,
        CartLineQuantityChanged, CartLineRemoved, CartLineRepriced, CartOpened, CartReopened,
        CartSaved, PromoCodeApplied, PromoCodeRemoved,
    },
    error::CartError,
    value_object::CartStatus,
};

pub struct Command<'a, E: Executor>(pub &'a E);

impl<E: Executor> Deref for Command<'_, E> {
    type Target = E;

    fn deref(&self) -> &Self::Target {
        self.0
    }
}

impl<E: Executor> Command<'_, E> {
    pub async fn load(&self, id: impl Into<String>) -> anyhow::Result<Option<CartState>> {
        create_projection().load(id).execute(self.0).await
    }

    /// Loads a cart that must exist and still be editable.
    async fn load_editable(&self, id: impl Into<String>) -> Result<CartState, CartError> {
        let Some(cart) = self.load(id).await? else {
            return Err(CartError::CartNotFound);
        };
        match cart.status {
            CartStatus::CheckedOut => Err(CartError::CartAlreadyCheckedOut),
            CartStatus::Discarded => Err(CartError::CartDiscarded),
            CartStatus::Open | CartStatus::Saved => Ok(cart),
        }
    }
}

/// Write-side state: just enough to guard the commands.
#[evento::projection(id = id)]
#[evento::snapshot(none)]
pub struct CartState {
    pub id: String,
    pub status: CartStatus,
    pub customer_id: Option<String>,
    pub products: Vec<String>,
    /// The unit price each line currently holds, as `(product_id, price)`.
    pub prices: Vec<(String, timada_core::Money)>,
    pub currency: Option<String>,
    pub has_promo_code: bool,
}

// Strict + explicit skips: a non-strict projection only *reads* the events it
// handles, so the version it observes (and `write()` relies on) would go stale.
fn create_projection<E: Executor>() -> Projection<E, CartState> {
    Projection::new::<Cart>()
        .handler(on_cart_opened())
        .handler(on_cart_line_added())
        .handler(on_cart_line_removed())
        .handler(on_cart_line_repriced())
        .handler(on_cart_saved())
        .handler(on_cart_assigned_to_customer())
        .handler(on_cart_reopened())
        .handler(on_cart_discarded())
        .handler(on_cart_checked_out())
        .handler(on_promo_code_applied())
        .handler(on_promo_code_removed())
        .skip::<CartLineQuantityChanged>()
        .strict()
}

#[evento::handler]
async fn on_cart_opened(event: Event<CartOpened>, row: &mut CartState) -> anyhow::Result<()> {
    row.id = event.aggregate_id.to_owned();
    row.customer_id = event.data.customer_id;
    Ok(())
}

#[evento::handler]
async fn on_cart_line_added(
    event: Event<CartLineAdded>,
    row: &mut CartState,
) -> anyhow::Result<()> {
    row.prices
        .push((event.data.product_id.clone(), event.data.unit_price.clone()));
    row.products.push(event.data.product_id);
    row.currency.get_or_insert(event.data.unit_price.currency);
    Ok(())
}

#[evento::handler]
async fn on_cart_line_removed(
    event: Event<CartLineRemoved>,
    row: &mut CartState,
) -> anyhow::Result<()> {
    row.products.retain(|p| p != &event.data.product_id);
    row.prices.retain(|(p, _)| p != &event.data.product_id);
    Ok(())
}

#[evento::handler]
async fn on_promo_code_applied(
    _event: Event<PromoCodeApplied>,
    row: &mut CartState,
) -> anyhow::Result<()> {
    row.has_promo_code = true;
    Ok(())
}

#[evento::handler]
async fn on_promo_code_removed(
    _event: Event<PromoCodeRemoved>,
    row: &mut CartState,
) -> anyhow::Result<()> {
    row.has_promo_code = false;
    Ok(())
}

#[evento::handler]
async fn on_cart_saved(_event: Event<CartSaved>, row: &mut CartState) -> anyhow::Result<()> {
    row.status = CartStatus::Saved;
    Ok(())
}

#[evento::handler]
async fn on_cart_checked_out(
    event: Event<CartCheckedOut>,
    row: &mut CartState,
) -> anyhow::Result<()> {
    row.status = CartStatus::CheckedOut;
    row.customer_id = Some(event.data.customer_id);
    Ok(())
}

#[evento::handler]
async fn on_cart_assigned_to_customer(
    event: Event<CartAssignedToCustomer>,
    row: &mut CartState,
) -> anyhow::Result<()> {
    row.customer_id = Some(event.data.customer_id);
    Ok(())
}

#[evento::handler]
async fn on_cart_reopened(_event: Event<CartReopened>, row: &mut CartState) -> anyhow::Result<()> {
    row.status = CartStatus::Open;
    Ok(())
}

#[evento::handler]
async fn on_cart_discarded(
    _event: Event<CartDiscarded>,
    row: &mut CartState,
) -> anyhow::Result<()> {
    row.status = CartStatus::Discarded;
    Ok(())
}

#[evento::handler]
async fn on_cart_line_repriced(
    event: Event<CartLineRepriced>,
    row: &mut CartState,
) -> anyhow::Result<()> {
    if let Some((_, price)) = row
        .prices
        .iter_mut()
        .find(|(product_id, _)| *product_id == event.data.product_id)
    {
        *price = event.data.unit_price;
    }
    Ok(())
}
