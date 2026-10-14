//! What this browser holds: its cart, the orders it placed (only those may
//! be looked at again) and the address it last checked out with. Each lives
//! in a private (encrypted, authenticated) cookie; the library itself keeps
//! no notion of who a shopper is.

use serde::{Deserialize, Serialize};
use timada_storefront::ShippingAddress;
use topcoat::{
    context::Cx,
    cookie::{Cookies, SameSite, UnparsedCookieStore, cookie_store, private_cookies, time},
};

/// How long a browser keeps its cart, its orders and its address.
const MAX_AGE_DAYS: i64 = 30;
/// How many of its orders a browser remembers.
const RECENT_ORDERS: usize = 10;

// Not `Secure`: the example is served over plain HTTP on 127.0.0.1. A shop
// behind HTTPS sets it (and may use a `__Host-` name).
fn jar(cx: &Cx) -> impl Cookies + '_ {
    private_cookies(cx)
        .default_same_site(SameSite::Lax)
        .default_path("/")
        .default_http_only(true)
        .default_max_age(time::Duration::days(MAX_AGE_DAYS))
}

fn cart_store(cx: &Cx) -> UnparsedCookieStore<String, impl Cookies + '_> {
    cookie_store(jar(cx), "cart")
}

fn orders_store(cx: &Cx) -> UnparsedCookieStore<Vec<String>, impl Cookies + '_> {
    cookie_store(jar(cx), "orders")
}

fn checkout_store(cx: &Cx) -> UnparsedCookieStore<CheckoutDraft, impl Cookies + '_> {
    cookie_store(jar(cx), "checkout")
}

/// The id of this browser's cart, if it has one.
pub fn cart_id(cx: &Cx) -> Option<String> {
    cart_store(cx)
        .parse()
        .ok()
        .flatten()
        .map(|store| store.get())
}

pub fn set_cart_id(cx: &Cx, cart_id: &str) -> topcoat::Result<()> {
    cart_store(cx).set(cart_id.to_owned()).commit()?;
    Ok(())
}

/// Starts the next cart afresh: the last one became an order.
pub fn forget_cart(cx: &Cx) {
    cart_store(cx).remove();
}

/// Whether this browser placed that order.
pub fn placed(cx: &Cx, order_id: &str) -> bool {
    recent_orders(cx).iter().any(|id| id == order_id)
}

fn recent_orders(cx: &Cx) -> Vec<String> {
    orders_store(cx)
        .parse()
        .ok()
        .flatten()
        .map(|store| store.get())
        .unwrap_or_default()
}

pub fn remember_order(cx: &Cx, order_id: &str) -> topcoat::Result<()> {
    let mut orders = recent_orders(cx);
    orders.retain(|id| id != order_id);
    orders.insert(0, order_id.to_owned());
    orders.truncate(RECENT_ORDERS);
    orders_store(cx).set(orders).commit()?;
    Ok(())
}

/// What the checkout form was last filled with, so a refused form or the
/// next order starts from it.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
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
}

impl CheckoutDraft {
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
}

pub fn checkout_draft(cx: &Cx) -> CheckoutDraft {
    checkout_store(cx)
        .parse()
        .ok()
        .flatten()
        .map(|store| store.get())
        .unwrap_or_default()
}

pub fn save_checkout_draft(cx: &Cx, draft: CheckoutDraft) -> topcoat::Result<()> {
    checkout_store(cx).set(draft).commit()?;
    Ok(())
}
