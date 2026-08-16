//! The cart read model, replayed from the cart's own events.
//!
//! This is the only cart read model there is — see the crate docs for why the
//! cart has no SQL projection table.

use evento::ProjectionAggregate;
use evento::metadata::Event;
use evento::projection::Projection;
use timada_core::{Executor, Money};

use crate::aggregate::{Cart, CartCheckedOut, CartItemAdded, CartItemRemoved};

/// One product in a cart, at the price and title it had when it was added.
#[derive(Debug, Clone, PartialEq, Eq, Default, bitcode::Encode, bitcode::Decode)]
pub struct CartLine {
    pub product_id: String,
    pub title: String,
    pub unit_price: Money,
    pub supplier_id: String,
    pub supplier_product_ref: String,
    pub quantity: u32,
}

impl CartLine {
    /// What this line costs: unit price times quantity.
    pub fn line_total(&self) -> Money {
        self.unit_price.multiply(self.quantity)
    }
}

/// A cart as the storefront shows it.
#[evento::projection(bitcode::Encode, bitcode::Decode)]
#[derive(Debug, PartialEq, Eq)]
pub struct CartView {
    pub id: String,
    pub lines: Vec<CartLine>,
    pub checked_out: bool,
}

impl ProjectionAggregate for CartView {
    fn aggregate_id(&self) -> String {
        self.id.to_owned()
    }
}

impl CartView {
    /// The cart's grand total.
    ///
    /// Every line in a cart shares one currency in practice — the catalog
    /// prices each product in its supplier's currency and the storefront is
    /// single-currency — so the currency of the first line is the cart's
    /// currency. A line that somehow disagrees is logged and left out of the
    /// total rather than crashing the cart page; the line itself still renders
    /// with its own price, so the discrepancy is visible instead of hidden.
    pub fn total(&self) -> Money {
        let currency = self
            .lines
            .first()
            .map(|line| line.unit_price.currency)
            .unwrap_or_default();

        self.lines
            .iter()
            .fold(Money::zero(currency), |total, line| {
                match total.add(line.line_total()) {
                    Ok(total) => total,
                    Err(error) => {
                        tracing::error!(
                            cart_id = %self.id,
                            product_id = %line.product_id,
                            %error,
                            "cart line currency does not match the cart, excluded from the total"
                        );
                        total
                    }
                }
            })
    }

    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    /// Total number of units in the cart, not the number of distinct lines.
    pub fn item_count(&self) -> u32 {
        self.lines
            .iter()
            .fold(0u32, |count, line| count.saturating_add(line.quantity))
    }

    fn line_mut(&mut self, product_id: &str) -> Option<&mut CartLine> {
        self.lines
            .iter_mut()
            .find(|line| line.product_id == product_id)
    }
}

/// Adding a product already in the cart raises its quantity instead of
/// producing a second line, and refreshes the title/price snapshot: the newer
/// event is the more recent thing the customer was shown and agreed to.
#[evento::handler]
async fn apply_item_added(event: Event<CartItemAdded>, cart: &mut CartView) -> anyhow::Result<()> {
    cart.id = event.aggregate_id.clone();

    match cart.line_mut(&event.data.product_id) {
        Some(line) => {
            line.title = event.data.title.clone();
            line.unit_price = event.data.unit_price;
            line.supplier_id = event.data.supplier_id.clone();
            line.supplier_product_ref = event.data.supplier_product_ref.clone();
            line.quantity = line.quantity.saturating_add(event.data.quantity);
        }
        None => cart.lines.push(CartLine {
            product_id: event.data.product_id.clone(),
            title: event.data.title.clone(),
            unit_price: event.data.unit_price,
            supplier_id: event.data.supplier_id.clone(),
            supplier_product_ref: event.data.supplier_product_ref.clone(),
            quantity: event.data.quantity,
        }),
    }

    Ok(())
}

#[evento::handler]
async fn apply_item_removed(
    event: Event<CartItemRemoved>,
    cart: &mut CartView,
) -> anyhow::Result<()> {
    cart.lines
        .retain(|line| line.product_id != event.data.product_id);
    Ok(())
}

#[evento::handler]
async fn apply_checked_out(
    _event: Event<CartCheckedOut>,
    cart: &mut CartView,
) -> anyhow::Result<()> {
    cart.checked_out = true;
    Ok(())
}

/// Replay one cart. `None` means the id has no events — an unknown or
/// never-used cart cookie.
pub async fn load_cart(executor: &Executor, cart_id: &str) -> anyhow::Result<Option<CartView>> {
    projection().load(cart_id).execute(executor).await
}

fn projection() -> Projection<Executor, CartView> {
    Projection::<Executor, CartView>::new::<Cart>()
        .handler(apply_item_added())
        .handler(apply_item_removed())
        .handler(apply_checked_out())
        .strict()
}
