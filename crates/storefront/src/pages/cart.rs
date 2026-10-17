//! The cart's page: what the browser is about to buy, priced from the
//! store as it stands. `?error=` carries a refusal.

use topcoat_core::context::Cx;
use topcoat_core::error::Result;

use crate::actions::query;
use crate::cart::CartView;
use crate::context::menu;
use crate::query::CategoryNode;
use crate::session::current_cart;
use crate::storefront::{money_format, paths};

use super::{Flash, LineModel, flash, line_models};

#[derive(serde::Deserialize)]
struct CartQuery {
    error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CartModel {
    pub nav: Vec<CategoryNode>,
    /// The cart, unless it is empty.
    pub cart: Option<CartView>,
    /// Its lines, those that can be sold first, then those off sale.
    pub lines: Vec<LineModel>,
    pub currency: String,
    pub subtotal: Option<String>,
    /// What the discount code takes off (`-2,50€`), when it does.
    pub discount_amount: Option<String>,
    /// Why the code takes nothing off, when it does not.
    pub discount_refused: Option<Flash>,
    pub total: Option<String>,
    pub can_check_out: bool,
    pub error: Option<Flash>,
    pub checkout_url: String,
    pub home_url: String,
}

pub async fn cart_page(cx: &Cx) -> Result<CartModel> {
    let query: CartQuery = query(cx)?;
    let nav = menu(cx).await?;
    let cart = current_cart(cx).await?.filter(|cart| !cart.is_empty());
    let currency = cart
        .as_ref()
        .and_then(|cart| cart.currency.clone())
        .unwrap_or_default();
    let money = money_format(cx);
    let paths = paths(cx);
    let lines = cart
        .as_ref()
        .map(|cart| line_models(cx, cart.lines.iter().chain(&cart.unavailable), &currency))
        .unwrap_or_default();
    Ok(CartModel {
        nav,
        lines,
        subtotal: cart
            .as_ref()
            .map(|cart| money.format(cart.subtotal_cents, &currency)),
        discount_amount: cart.as_ref().and_then(|cart| {
            cart.discount
                .as_ref()
                .map(|discount| money.signed(-(discount.amount_cents as i64), &currency))
        }),
        discount_refused: flash(cx, cart.as_ref().and_then(|cart| cart.discount_refused)),
        total: cart
            .as_ref()
            .map(|cart| money.format(cart.total_cents, &currency)),
        can_check_out: cart.as_ref().is_some_and(CartView::can_check_out),
        error: flash(cx, query.error.as_deref()),
        checkout_url: paths.checkout_url(None),
        home_url: paths.home_url(),
        currency,
        cart,
    })
}
