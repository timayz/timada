//! What every page's frame shows: the menu, the cart's count and total in
//! the header, who is signed in.

use topcoat_core::context::Cx;
use topcoat_core::error::Result;

use crate::context::menu;
use crate::query::CategoryNode;
use crate::session::{accounts_enabled, browser_cart, signed_in};
use crate::storefront::{money_format, paths};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellModel {
    /// The categories the menu shows.
    pub nav: Vec<CategoryNode>,
    /// Items in the browser's cart.
    pub cart_count: u32,
    /// What the cart's lines add up to, when it has any.
    pub cart_subtotal: Option<String>,
    /// What the goods cost after the discount, when the cart has any.
    pub cart_total: Option<String>,
    /// Whether the shop signs shoppers in.
    pub accounts_enabled: bool,
    pub signed_in: bool,
    pub shopper_email: Option<String>,
    pub home_url: String,
    pub catalog_url: String,
    pub cart_url: String,
    pub account_url: String,
    pub sign_in_url: String,
}

pub async fn shell(cx: &Cx) -> Result<ShellModel> {
    let nav = menu(cx).await?;
    let cart = browser_cart(cx).await;
    let money = money_format(cx);
    let paths = paths(cx);
    let accounts = accounts_enabled(cx);
    let shopper = if accounts { signed_in(cx).await } else { None };
    Ok(ShellModel {
        nav,
        cart_count: cart.map(|cart| cart.item_count).unwrap_or_default(),
        cart_subtotal: cart.and_then(|cart| {
            cart.currency
                .as_deref()
                .map(|currency| money.format(cart.subtotal_cents, currency))
        }),
        cart_total: cart.and_then(|cart| {
            cart.currency
                .as_deref()
                .map(|currency| money.format(cart.total_cents, currency))
        }),
        accounts_enabled: accounts,
        signed_in: shopper.is_some(),
        shopper_email: shopper.map(|shopper| shopper.email.clone()),
        home_url: paths.home_url(),
        catalog_url: paths.catalog_url(None, 1),
        cart_url: paths.cart_url(None),
        account_url: paths.account_url(None),
        sign_in_url: paths.sign_in_url(None, None),
    })
}
