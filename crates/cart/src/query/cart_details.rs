//! Everything "votre panier" needs, folded from one `Cart` stream.
//! Executor-backed snapshots via the bitcode derives.

use evento::{Executor, metadata::Event, projection::Projection};
use timada_core::Money;

use crate::{
    aggregator::{
        Cart, CartCheckedOut, CartLineAdded, CartLineQuantityChanged, CartLineRemoved, CartOpened,
        CartSaved, PromoCodeApplied,
    },
    value_object::{CartLine, CartStatus},
};

#[evento::projection(bitcode::Encode, bitcode::Decode)]
#[derive(Debug, PartialEq)]
pub struct CartDetailsView {
    pub id: String,
    pub customer_id: Option<String>,
    pub lines: Vec<CartLine>,
    pub promo_code: Option<String>,
    pub subtotal: Money,
    pub status: CartStatus,
    pub saved_name: Option<String>,
}

impl CartDetailsView {
    fn recompute_subtotal(&mut self) -> anyhow::Result<()> {
        let currency = self
            .lines
            .first()
            .map(|l| l.unit_price.currency.clone())
            .unwrap_or_else(|| Money::EUR.to_owned());
        let mut subtotal = Money::zero(currency);
        for line in &self.lines {
            subtotal = subtotal.checked_add(&line.unit_price.checked_mul(line.quantity)?)?;
        }
        self.subtotal = subtotal;
        Ok(())
    }
}

pub fn create_projection<E: Executor>() -> Projection<E, CartDetailsView> {
    Projection::new::<Cart>()
        .handler(on_cart_opened())
        .handler(on_cart_line_added())
        .handler(on_cart_line_quantity_changed())
        .handler(on_cart_line_removed())
        .handler(on_promo_code_applied())
        .handler(on_cart_saved())
        .handler(on_cart_checked_out())
        .strict()
}

pub async fn load<E: Executor>(
    executor: &E,
    id: impl Into<String>,
) -> anyhow::Result<Option<CartDetailsView>> {
    create_projection().load(id).execute(executor).await
}

#[evento::handler]
async fn on_cart_opened(event: Event<CartOpened>, row: &mut CartDetailsView) -> anyhow::Result<()> {
    row.id = event.aggregate_id.to_owned();
    row.customer_id = event.data.customer_id;
    Ok(())
}

#[evento::handler]
async fn on_cart_line_added(
    event: Event<CartLineAdded>,
    row: &mut CartDetailsView,
) -> anyhow::Result<()> {
    row.lines.push(CartLine {
        product_id: event.data.product_id,
        name: event.data.name,
        quantity: event.data.quantity,
        unit_price: event.data.unit_price,
        warranty_months: event.data.warranty_months,
    });
    row.recompute_subtotal()
}

#[evento::handler]
async fn on_cart_line_quantity_changed(
    event: Event<CartLineQuantityChanged>,
    row: &mut CartDetailsView,
) -> anyhow::Result<()> {
    if let Some(line) = row
        .lines
        .iter_mut()
        .find(|l| l.product_id == event.data.product_id)
    {
        line.quantity = event.data.quantity;
    }
    row.recompute_subtotal()
}

#[evento::handler]
async fn on_cart_line_removed(
    event: Event<CartLineRemoved>,
    row: &mut CartDetailsView,
) -> anyhow::Result<()> {
    row.lines.retain(|l| l.product_id != event.data.product_id);
    row.recompute_subtotal()
}

#[evento::handler]
async fn on_promo_code_applied(
    event: Event<PromoCodeApplied>,
    row: &mut CartDetailsView,
) -> anyhow::Result<()> {
    row.promo_code = Some(event.data.code);
    Ok(())
}

#[evento::handler]
async fn on_cart_saved(event: Event<CartSaved>, row: &mut CartDetailsView) -> anyhow::Result<()> {
    row.status = CartStatus::Saved;
    row.saved_name = Some(event.data.name);
    Ok(())
}

#[evento::handler]
async fn on_cart_checked_out(
    event: Event<CartCheckedOut>,
    row: &mut CartDetailsView,
) -> anyhow::Result<()> {
    row.status = CartStatus::CheckedOut;
    row.customer_id = Some(event.data.customer_id);
    Ok(())
}
