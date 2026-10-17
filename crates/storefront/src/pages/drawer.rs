//! The cart drawer: the cart sliding in on every page, with what goes well
//! with it beside. `?cart_error=` carries a refusal from one of its forms.

use topcoat_core::context::Cx;
use topcoat_core::error::Result;

use crate::actions::query;
use crate::cart::{CartView, payment_setup};
use crate::context::cart_recommendations;
use crate::query::Product;
use crate::session::browser_cart;
use crate::storefront::{actions, messages, money_format, paths, storefront};
use crate::urls::{current_path, drawer_url};

use super::{Flash, LineModel, flash, line_models};

#[derive(serde::Deserialize)]
struct DrawerQuery {
    cart_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DrawerOptions {
    /// How many products the "goes well with" column offers.
    pub offered: u32,
}

impl Default for DrawerOptions {
    fn default() -> Self {
        Self { offered: 6 }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CartDrawerModel {
    /// The cart, unless it is empty.
    pub cart: Option<CartView>,
    pub lines: Vec<LineModel>,
    /// Where the drawer's forms come back to: this page, as the shopper
    /// sees it, without the refusal it may already show.
    pub from: String,
    pub count: u32,
    pub currency: String,
    pub subtotal: String,
    pub discount_amount: Option<String>,
    pub discount_refused: Option<Flash>,
    pub total: String,
    pub total_cents: u64,
    /// The provider's public key, when the cart can be paid for.
    pub publishable_key: String,
    pub has_payments: bool,
    pub can_check_out: bool,
    /// The countries a wallet may ship to, comma-separated.
    pub countries: String,
    /// The shop's shipping methods as a wallet offers them, JSON.
    pub rates: String,
    /// How the script writes money, JSON.
    pub money: String,
    pub error: Option<Flash>,
    /// Whether to leave the promo box open: a code is on, or one was refused.
    pub promo_open: bool,
    pub offered: Vec<Product>,
    pub checkout_url: String,
    pub home_url: String,
    pub start_url: String,
    pub return_url: String,
    pub accent_color: String,
    pub standard_shipping: String,
    pub something_went_wrong: String,
}

pub async fn cart_drawer(cx: &Cx, options: &DrawerOptions) -> Result<CartDrawerModel> {
    let query: DrawerQuery = query(cx)?;
    let from = drawer_url(&current_path(cx), None)
        .trim_end_matches("#cart")
        .to_owned();
    let cart = browser_cart(cx)
        .await
        .filter(|cart| !cart.is_empty())
        .cloned();
    let offered = match &cart {
        Some(cart) => cart_recommendations(cx, cart, options.offered).await?,
        None => Vec::new(),
    };
    let setup = match &cart {
        Some(cart) if cart.can_check_out() => payment_setup(cx).await?,
        _ => None,
    };
    let money = money_format(cx);
    let paths = paths(cx);
    let messages = messages(cx);
    let currency = cart
        .as_ref()
        .and_then(|cart| cart.currency.clone())
        .unwrap_or_default();
    let lines = cart
        .as_ref()
        .map(|cart| line_models(cx, cart.lines.iter().chain(&cart.unavailable), &currency))
        .unwrap_or_default();
    let error = flash(cx, query.cart_error.as_deref());
    let promo_refused = error
        .as_ref()
        .is_some_and(|error| error.code.starts_with("discount_"));
    Ok(CartDrawerModel {
        lines,
        from,
        count: cart
            .as_ref()
            .map(|cart| cart.item_count)
            .unwrap_or_default(),
        subtotal: cart
            .as_ref()
            .map(|cart| money.format(cart.subtotal_cents, &currency))
            .unwrap_or_default(),
        discount_amount: cart.as_ref().and_then(|cart| {
            cart.discount
                .as_ref()
                .map(|discount| money.signed(-(discount.amount_cents as i64), &currency))
        }),
        discount_refused: flash(cx, cart.as_ref().and_then(|cart| cart.discount_refused)),
        total: cart
            .as_ref()
            .map(|cart| money.format(cart.total_cents, &currency))
            .unwrap_or_default(),
        total_cents: cart
            .as_ref()
            .map(|cart| cart.total_cents)
            .unwrap_or_default(),
        publishable_key: setup
            .as_ref()
            .map(|setup| setup.publishable_key.clone())
            .unwrap_or_default(),
        has_payments: setup.is_some(),
        can_check_out: cart.as_ref().is_some_and(CartView::can_check_out),
        countries: messages.country_codes(),
        rates: cart
            .as_ref()
            .map(super::express_rates)
            .unwrap_or_else(|| "[]".to_owned()),
        money: money.descriptor(&currency),
        promo_open: cart
            .as_ref()
            .is_some_and(|cart| cart.discount_code.is_some())
            || promo_refused,
        error,
        offered,
        checkout_url: paths.checkout_url(None),
        home_url: paths.home_url(),
        start_url: actions(cx).checkout_start(),
        return_url: paths.checkout_complete_url(),
        accent_color: storefront(cx).accent_color.clone().unwrap_or_default(),
        standard_shipping: messages.standard_shipping.to_string(),
        something_went_wrong: messages.something_went_wrong.to_string(),
        currency,
        cart,
    })
}
