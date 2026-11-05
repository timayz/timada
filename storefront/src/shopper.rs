//! What this browser holds: its cart, the orders it placed (only those, and
//! the signed-in shopper's, may be looked at again), the address it last
//! checked out with, and the token of the shopper signed in on it. Each lives
//! in a private (encrypted, authenticated) cookie.

use serde::{Deserialize, Serialize};
use timada_storefront::{SavedAddress, ShippingAddress, Shopper, order, shopper, store};

use crate::app::checkout::countries;
use topcoat::{
    context::{Cx, memoize},
    cookie::{Cookies, SameSite, UnparsedCookieStore, cookie_store, private_cookies, time},
};

/// How long a browser keeps its cart, its orders and its address.
const MAX_AGE_DAYS: i64 = 30;
/// How many of its orders a browser remembers.
const RECENT_ORDERS: usize = 10;

// Not `Secure`: the app is served over plain HTTP on 127.0.0.1. A shop
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
/// Every field has a default: the form posts only what it shows (a shop
/// without businesses, methods or a billing fieldset posts none of theirs,
/// and the number comes as `phone_code` + `phone_number`, `phone` whole
/// only from a wallet).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct CheckoutDraft {
    pub email: String,
    pub name: String,
    pub line1: String,
    pub line2: String,
    pub postal_code: String,
    pub city: String,
    /// The province picked in the list a forwarding shop shows.
    #[serde(default)]
    pub region: String,
    pub country: String,
    /// The phone as ordered (`+33 6 12 34 56 78`): what the form's two
    /// fields compose, or what a wallet handed over whole.
    pub phone: String,
    /// The form's calling code (`33`) and national number, as typed.
    #[serde(default)]
    pub phone_code: String,
    #[serde(default)]
    pub phone_number: String,
    pub note: String,
    /// A business buyer's EU VAT number. The form has no such field in a
    /// shop that does not sell to businesses.
    #[serde(default)]
    pub vat_number: String,
    /// Not empty when the shopper is billed at another address than the
    /// delivery one; that address follows. Unticked, the billing fieldset is
    /// disabled and the browser sends none of it.
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
    /// The shipping method picked; the form has no methods in a shop that
    /// ships for free.
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
        self.region = address.region.clone().unwrap_or_default();
        self.country = address.country.clone();
        self.phone = address.phone.clone().unwrap_or_default();
        self.phone_code.clear();
        self.phone_number.clear();
        self
    }

    /// The phone as ordered: the form's two fields composed when the
    /// shopper typed a number, else the one handed over whole.
    pub fn phone_text(&self) -> String {
        if self.phone_number.trim().is_empty() {
            self.phone.trim().to_owned()
        } else {
            compose_phone(&self.phone_code, &self.phone_number)
        }
    }

    /// What the form's two phone fields show: the calling code and the
    /// national number, from the fields as typed or from a whole phone;
    /// the delivery country's code, or France's, before any.
    pub fn phone_parts(&self) -> (String, String) {
        if !self.phone_number.trim().is_empty() && !self.phone_code.is_empty() {
            return (self.phone_code.clone(), self.phone_number.clone());
        }
        let fallback = || {
            countries::dial_code_of(&self.country.to_ascii_uppercase())
                .unwrap_or("33")
                .to_owned()
        };
        match split_phone(&self.phone) {
            (Some(code), national) => (code.to_owned(), national),
            (None, national) => (fallback(), national),
        }
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
            region: optional(&self.region),
            country: self.country.clone(),
            phone: optional(&self.phone_text()),
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
            region: None,
            country: self.billing_country.clone(),
            phone: None,
        })
    }
}

/// A calling code and a national number as one phone: `+33 6 12 34 56 78`.
/// The trunk `0` most countries dial at home goes (Italy keeps it); a
/// number typed international already is kept whole.
pub fn compose_phone(code: &str, national: &str) -> String {
    let national = national.trim();
    if national.is_empty() {
        return String::new();
    }
    if national.starts_with('+') || national.starts_with("00") {
        return national.to_owned();
    }
    let code = code.trim().trim_start_matches('+');
    let national = match national.strip_prefix('0') {
        Some(rest) if code != "39" => rest.trim_start(),
        _ => national,
    };
    format!("+{code} {national}").trim_end().to_owned()
}

/// A phone split into the calling code it starts with (`+33`, `0033`), when
/// it is one of the shop's countries', and the rest.
pub fn split_phone(phone: &str) -> (Option<&'static str>, String) {
    let phone = phone.trim();
    let Some(international) = phone.strip_prefix('+').or_else(|| phone.strip_prefix("00")) else {
        return (None, phone.to_owned());
    };
    let mut codes: Vec<&'static str> = countries::DIAL_CODES
        .iter()
        .map(|(_, code)| *code)
        .collect();
    codes.sort_by_key(|code| std::cmp::Reverse(code.len()));
    codes.dedup();
    let international = international.trim_start();
    match codes.iter().find(|code| international.starts_with(*code)) {
        Some(code) => (Some(code), international[code.len()..].trim().to_owned()),
        None => (None, phone.to_owned()),
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
pub async fn signed_in(cx: &Cx) -> Option<Shopper> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_form_posts_the_phone_in_two_fields_and_a_wallet_whole() {
        let typed: CheckoutDraft = serde_urlencoded::from_str(
            "email=ada%40example.org&name=Ada&line1=1+rue&postal_code=75002&city=Paris&country=FR&phone_code=33&phone_number=06+12+34+56+78&note=",
        )
        .unwrap();
        assert_eq!(typed.phone_text(), "+33 6 12 34 56 78");
        let wallet: CheckoutDraft =
            serde_urlencoded::from_str("email=a%40b.c&name=Ada&phone=%2B33612345678&country=FR")
                .unwrap();
        assert_eq!(wallet.phone_text(), "+33612345678");
    }

    #[test]
    fn a_phone_is_composed_from_its_code_and_split_back() {
        assert_eq!(compose_phone("33", "06 12 34 56 78"), "+33 6 12 34 56 78");
        assert_eq!(compose_phone("39", "06 1234 5678"), "+39 06 1234 5678");
        assert_eq!(compose_phone("33", "+32 470 12 34 56"), "+32 470 12 34 56");
        assert_eq!(compose_phone("33", "  "), "");
        assert_eq!(
            split_phone("+33 6 12 34 56 78"),
            (Some("33"), "6 12 34 56 78".into())
        );
        assert_eq!(
            split_phone("0032470123456"),
            (Some("32"), "470123456".into())
        );
        assert_eq!(split_phone("+359 2 123"), (Some("359"), "2 123".into()));
        assert_eq!(
            split_phone("06 12 34 56 78"),
            (None, "06 12 34 56 78".into())
        );
        assert_eq!(split_phone("+999 1"), (None, "+999 1".into()));

        let draft = CheckoutDraft {
            country: "be".into(),
            phone: "+33 6 12 34 56 78".into(),
            ..Default::default()
        };
        assert_eq!(draft.phone_parts(), ("33".into(), "6 12 34 56 78".into()));
        assert_eq!(draft.phone_text(), "+33 6 12 34 56 78");
        let typed = CheckoutDraft {
            phone_code: "32".into(),
            phone_number: "0470 12 34 56".into(),
            ..draft.clone()
        };
        assert_eq!(typed.phone_parts(), ("32".into(), "0470 12 34 56".into()));
        assert_eq!(typed.phone_text(), "+32 470 12 34 56");
        let blank = CheckoutDraft {
            country: "be".into(),
            ..Default::default()
        };
        assert_eq!(blank.phone_parts(), ("32".into(), String::new()));
        assert_eq!(blank.phone_text(), "");
    }
}
