mod add_line;
mod apply_promo_code;
mod change_line_quantity;
mod checkout;
mod open_cart;
mod remove_line;
mod save_cart;

use std::ops::Deref;

pub use add_line::AddLine;
pub use checkout::Checkout;

use evento::{Executor, Projection, metadata::Event};

use crate::{
    aggregator::{
        Cart, CartCheckedOut, CartLineAdded, CartLineQuantityChanged, CartLineRemoved, CartOpened,
        CartSaved, PromoCodeApplied,
    },
    error::CartError,
    value_object::CartStatus,
};

pub struct Command<E: Executor>(pub E);

impl<E: Executor> Deref for Command<E> {
    type Target = E;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<E: Executor> Command<E> {
    pub async fn load(&self, id: impl Into<String>) -> anyhow::Result<Option<CartState>> {
        create_projection().load(id).execute(&self.0).await
    }

    /// Loads a cart that must exist and still be editable.
    async fn load_editable(&self, id: impl Into<String>) -> Result<CartState, CartError> {
        let Some(cart) = self.load(id).await? else {
            return Err(CartError::CartNotFound);
        };
        if cart.status == CartStatus::CheckedOut {
            return Err(CartError::CartAlreadyCheckedOut);
        }
        Ok(cart)
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
    pub currency: Option<String>,
}

// Strict + explicit skips: a non-strict projection only *reads* the events it
// handles, so the version it observes (and `write()` relies on) would go stale.
fn create_projection<E: Executor>() -> Projection<E, CartState> {
    Projection::new::<Cart>()
        .handler(on_cart_opened())
        .handler(on_cart_line_added())
        .handler(on_cart_line_removed())
        .handler(on_cart_saved())
        .handler(on_cart_checked_out())
        .skip::<CartLineQuantityChanged>()
        .skip::<PromoCodeApplied>()
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
