//! What a shopper's browser holds: its cart, the orders it placed (only
//! those, and the signed-in shopper's, may be looked at again), the address
//! it last checked out with, the payment going through, and the token of
//! the shopper signed in on it. Each lives in a private (encrypted,
//! authenticated) cookie sealed with the key the app gave
//! [`Storefront::cookie_key`](crate::Storefront::cookie_key).

use serde::{Deserialize, Serialize};
use topcoat_cookie::{Cookies, UnparsedCookieStore, cookie_store, private_cookies, time};
use topcoat_core::context::Cx;
use topcoat_core::error::Result;
use topcoat_core_macro::memoize;

use crate::cart::{CartView, ShippingAddress, cart, order};
use crate::shoppers::{SavedAddress, Shopper, shopper};
use crate::storefront::storefront;

/// How many of its orders a browser remembers.
const RECENT_ORDERS: usize = 10;

fn jar(cx: &Cx) -> impl Cookies + '_ {
    let options = &storefront(cx).cookies;
    private_cookies(cx)
        .default_same_site(options.same_site)
        .default_path(options.path.to_string())
        .default_http_only(options.http_only)
        .default_secure(options.secure)
        .default_max_age(time::Duration::days(options.max_age_days))
}

fn name(cx: &Cx, base: &str) -> String {
    format!("{}{base}", storefront(cx).cookies.name_prefix)
}

fn store_of<'cx, T>(cx: &'cx Cx, base: &str) -> UnparsedCookieStore<T, impl Cookies + 'cx>
where
    T: Serialize + serde::de::DeserializeOwned,
{
    cookie_store(jar(cx), name(cx, base))
}

fn read<T: serde::de::DeserializeOwned + Serialize + Clone>(cx: &Cx, base: &str) -> Option<T> {
    store_of::<T>(cx, base)
        .parse()
        .ok()
        .flatten()
        .map(|store| store.get())
}

fn write<T: serde::de::DeserializeOwned + Serialize>(cx: &Cx, base: &str, value: T) -> Result<()> {
    store_of::<T>(cx, base).set(value).commit()?;
    Ok(())
}

// =============================================================================
// The cart
// =============================================================================

/// The id of this browser's cart, if it has one.
pub fn cart_id(cx: &Cx) -> Option<String> {
    read(cx, "cart")
}

pub fn set_cart_id(cx: &Cx, cart_id: &str) -> Result<()> {
    write(cx, "cart", cart_id.to_owned())
}

/// Starts the next cart afresh: the last one became an order.
pub fn forget_cart(cx: &Cx) {
    store_of::<String>(cx, "cart").remove();
}

/// This browser's cart, unless it has none or it became an order.
pub async fn current_cart(cx: &Cx) -> Result<Option<CartView>> {
    let Some(id) = cart_id(cx) else {
        return Ok(None);
    };
    Ok(cart(cx, &id).await?.filter(|cart| cart.order_id.is_none()))
}

/// [`current_cart`] read once per request, for the header and the drawer
/// that show on every page: a store that fails to answer reads as no cart,
/// and is logged, rather than failing the page.
#[memoize(as_ref)]
pub async fn browser_cart(cx: &Cx) -> Option<CartView> {
    match current_cart(cx).await {
        Ok(cart) => cart,
        Err(err) => {
            tracing::error!(%err, "cannot read the browser's cart");
            None
        }
    }
}

// =============================================================================
// The orders placed
// =============================================================================

fn recent_orders(cx: &Cx) -> Vec<String> {
    read(cx, "orders").unwrap_or_default()
}

/// Whether this browser placed that order.
pub fn placed(cx: &Cx, order_id: &str) -> bool {
    recent_orders(cx).iter().any(|id| id == order_id)
}

/// Whether this browser may look at that order: it placed it, or the
/// shopper signed in on it did (with the same e-mail address, guest orders
/// included).
pub async fn holds_order(cx: &Cx, order_id: &str) -> Result<bool> {
    if placed(cx, order_id) {
        return Ok(true);
    }
    let Some(shopper) = signed_in(cx).await else {
        return Ok(false);
    };
    let email = shopper.email.clone();
    Ok(order(cx, order_id)
        .await?
        .is_some_and(|order| order.email == email))
}

pub fn remember_order(cx: &Cx, order_id: &str) -> Result<()> {
    let mut orders = recent_orders(cx);
    orders.retain(|id| id != order_id);
    orders.insert(0, order_id.to_owned());
    orders.truncate(RECENT_ORDERS);
    write(cx, "orders", orders)
}

// =============================================================================
// The checkout form
// =============================================================================

/// What the checkout form was last filled with, so a refused form or the
/// next order starts from it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckoutDraft {
    pub email: String,
    pub name: String,
    pub line1: String,
    pub line2: String,
    pub postal_code: String,
    pub city: String,
    pub country: String,
    pub phone: String,
    pub note: String,
    /// A business buyer's EU VAT number; absent from drafts kept before it
    /// existed.
    #[serde(default)]
    pub vat_number: String,
    /// Not empty when the shopper is billed at another address than the
    /// delivery one; that address follows. All absent from older drafts.
    #[serde(default)]
    pub billing_different: String,
    #[serde(default)]
    pub billing_name: String,
    #[serde(default)]
    pub billing_line1: String,
    #[serde(default)]
    pub billing_line2: String,
    #[serde(default)]
    pub billing_postal_code: String,
    #[serde(default)]
    pub billing_city: String,
    #[serde(default)]
    pub billing_country: String,
    /// The shipping method picked; absent from older drafts and in a shop
    /// that ships for free.
    #[serde(default)]
    pub shipping_method: String,
}

impl CheckoutDraft {
    /// The draft with a saved address in place of the one typed last.
    pub fn with_address(mut self, saved: &SavedAddress) -> Self {
        let address = &saved.address;
        self.name = address.name.clone();
        self.line1 = address.line1.clone();
        self.line2 = address.line2.clone().unwrap_or_default();
        self.postal_code = address.postal_code.clone();
        self.city = address.city.clone();
        self.country = address.country.clone();
        self.phone = address.phone.clone().unwrap_or_default();
        self
    }

    /// The draft billed at a saved address.
    pub fn with_billing_address(mut self, saved: &SavedAddress) -> Self {
        let address = &saved.address;
        self.billing_different = "1".to_owned();
        self.billing_name = address.name.clone();
        self.billing_line1 = address.line1.clone();
        self.billing_line2 = address.line2.clone().unwrap_or_default();
        self.billing_postal_code = address.postal_code.clone();
        self.billing_city = address.city.clone();
        self.billing_country = address.country.clone();
        self
    }

    pub fn shipping(&self) -> ShippingAddress {
        let optional = |value: &str| (!value.trim().is_empty()).then(|| value.to_owned());
        ShippingAddress {
            name: self.name.clone(),
            line1: self.line1.clone(),
            line2: optional(&self.line2),
            postal_code: self.postal_code.clone(),
            city: self.city.clone(),
            country: self.country.clone(),
            phone: optional(&self.phone),
        }
    }

    /// Whether the shopper ticked "a different billing address".
    pub fn bills_elsewhere(&self) -> bool {
        !self.billing_different.is_empty()
    }

    /// Where to bill, when not at the delivery address.
    pub fn billing(&self) -> Option<ShippingAddress> {
        let optional = |value: &str| (!value.trim().is_empty()).then(|| value.to_owned());
        self.bills_elsewhere().then(|| ShippingAddress {
            name: self.billing_name.clone(),
            line1: self.billing_line1.clone(),
            line2: optional(&self.billing_line2),
            postal_code: self.billing_postal_code.clone(),
            city: self.billing_city.clone(),
            country: self.billing_country.clone(),
            phone: None,
        })
    }
}

pub fn checkout_draft(cx: &Cx) -> CheckoutDraft {
    read(cx, "checkout").unwrap_or_default()
}

pub fn save_checkout_draft(cx: &Cx, draft: CheckoutDraft) -> Result<()> {
    write(cx, "checkout", draft)
}

pub fn forget_checkout_draft(cx: &Cx) {
    store_of::<CheckoutDraft>(cx, "checkout").remove();
}

// =============================================================================
// The payment going through
// =============================================================================

/// The request to pay this browser is going through: the return page reads
/// its order back by it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaymentInProgress {
    pub cart_id: String,
    pub request_id: String,
}

pub fn payment_in_progress(cx: &Cx) -> Option<PaymentInProgress> {
    read(cx, "payment")
}

pub fn set_payment_in_progress(cx: &Cx, payment: PaymentInProgress) -> Result<()> {
    write(cx, "payment", payment)
}

pub fn forget_payment(cx: &Cx) {
    store_of::<PaymentInProgress>(cx, "payment").remove();
}

// =============================================================================
// The shopper signed in
// =============================================================================

/// The token of the shopper signed in on this browser, if any.
pub fn shopper_token(cx: &Cx) -> Option<String> {
    read(cx, "shopper")
}

pub fn set_shopper_token(cx: &Cx, token: &str) -> Result<()> {
    write(cx, "shopper", token.to_owned())
}

pub fn forget_shopper(cx: &Cx) {
    store_of::<String>(cx, "shopper").remove();
}

/// The shopper signed in on this browser, read from the store once per
/// request. A token that no longer signs anyone in reads as signed out.
#[memoize(as_ref)]
pub async fn signed_in(cx: &Cx) -> Option<Shopper> {
    if !accounts_enabled(cx) {
        return None;
    }
    let token = shopper_token(cx)?;
    match shopper(cx, &token).await {
        Ok(shopper) => shopper,
        Err(err) => {
            tracing::error!(%err, "cannot read who is signed in");
            None
        }
    }
}

/// Whether this shop can sign shoppers in: it knows the dashboard's address.
pub fn accounts_enabled(cx: &Cx) -> bool {
    storefront(cx).store.signs_shoppers_in()
}

/// A sign-in on its way: the address the code went to, and where to go once
/// it is entered.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignInFlow {
    pub email: String,
    pub next: String,
}

fn sign_in_store(cx: &Cx) -> UnparsedCookieStore<SignInFlow, impl Cookies + '_> {
    let options = &storefront(cx).cookies;
    cookie_store(
        private_cookies(cx)
            .default_same_site(options.same_site)
            .default_path(options.path.to_string())
            .default_http_only(options.http_only)
            .default_secure(options.secure)
            // A code lives ten minutes; a little more to type it.
            .default_max_age(time::Duration::minutes(options.sign_in_minutes)),
        name(cx, "sign_in"),
    )
}

pub fn sign_in_flow(cx: &Cx) -> Option<SignInFlow> {
    sign_in_store(cx)
        .parse()
        .ok()
        .flatten()
        .map(|store| store.get())
}

pub fn set_sign_in_flow(cx: &Cx, flow: SignInFlow) -> Result<()> {
    sign_in_store(cx).set(flow).commit()?;
    Ok(())
}

pub fn forget_sign_in_flow(cx: &Cx) {
    sign_in_store(cx).remove();
}
