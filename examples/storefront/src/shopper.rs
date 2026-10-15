//! What this browser holds: its cart, the orders it placed (only those, and
//! the signed-in shopper's, may be looked at again), the address it last
//! checked out with, and the token of the shopper signed in on it. Each lives
//! in a private (encrypted, authenticated) cookie.

use serde::{Deserialize, Serialize};
use timada_storefront::{SavedAddress, ShippingAddress, ShopperState, order, shopper, store};
use topcoat::{
    context::{Cx, memoize},
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

/// Whether this browser may look at that order: it placed it, or the
/// shopper signed in on it did (with the same e-mail address, guest orders
/// included).
pub async fn holds_order(cx: &Cx, order_id: &str) -> topcoat::Result<bool> {
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

pub fn forget_checkout_draft(cx: &Cx) {
    checkout_store(cx).remove();
}

/// The request to pay this browser is going through: the card form's page
/// reads its payment back by it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaymentInProgress {
    pub cart_id: String,
    pub request_id: String,
}

fn payment_store(cx: &Cx) -> UnparsedCookieStore<PaymentInProgress, impl Cookies + '_> {
    cookie_store(jar(cx), "payment")
}

pub fn payment_in_progress(cx: &Cx) -> Option<PaymentInProgress> {
    payment_store(cx)
        .parse()
        .ok()
        .flatten()
        .map(|store| store.get())
}

pub fn set_payment_in_progress(cx: &Cx, payment: PaymentInProgress) -> topcoat::Result<()> {
    payment_store(cx).set(payment).commit()?;
    Ok(())
}

pub fn forget_payment(cx: &Cx) {
    payment_store(cx).remove();
}

fn shopper_store(cx: &Cx) -> UnparsedCookieStore<String, impl Cookies + '_> {
    cookie_store(jar(cx), "shopper")
}

/// The token of the shopper signed in on this browser, if any.
pub fn shopper_token(cx: &Cx) -> Option<String> {
    shopper_store(cx)
        .parse()
        .ok()
        .flatten()
        .map(|store| store.get())
}

pub fn set_shopper_token(cx: &Cx, token: &str) -> topcoat::Result<()> {
    shopper_store(cx).set(token.to_owned()).commit()?;
    Ok(())
}

pub fn forget_shopper(cx: &Cx) {
    shopper_store(cx).remove();
}

/// The shopper signed in on this browser, read from the store once per
/// request. A token that no longer signs anyone in reads as signed out.
#[memoize(as_ref)]
pub async fn signed_in(cx: &Cx) -> Option<ShopperState> {
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
    store(cx).signs_shoppers_in()
}

/// A sign-in on its way: the address the code went to, and where to go once
/// it is entered.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SignInFlow {
    pub email: String,
    pub next: String,
}

fn sign_in_store(cx: &Cx) -> UnparsedCookieStore<SignInFlow, impl Cookies + '_> {
    cookie_store(
        private_cookies(cx)
            .default_same_site(SameSite::Lax)
            .default_path("/")
            .default_http_only(true)
            // A code lives ten minutes; a little more to type it.
            .default_max_age(time::Duration::minutes(15)),
        "sign_in",
    )
}

pub fn sign_in_flow(cx: &Cx) -> Option<SignInFlow> {
    sign_in_store(cx)
        .parse()
        .ok()
        .flatten()
        .map(|store| store.get())
}

pub fn set_sign_in_flow(cx: &Cx, flow: SignInFlow) -> topcoat::Result<()> {
    sign_in_store(cx).set(flow).commit()?;
    Ok(())
}

pub fn forget_sign_in_flow(cx: &Cx) {
    sign_in_store(cx).remove();
}

/// Where to go after signing in: a path on this site, else the account.
pub fn safe_next(next: Option<&str>, fallback: String) -> String {
    match next {
        Some(next) if next.starts_with('/') && !next.starts_with("//") && !next.contains('\\') => {
            next.to_owned()
        }
        _ => fallback,
    }
}
