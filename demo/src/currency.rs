//! Which currency a browser shops in. The shop sells in a few
//! ([`crate::db::shop_currencies`]); the shopper's choice travels in a cookie.
//!
//! A cart being filled has the last word: its lines were priced in one
//! currency and a cart never mixes two, so as long as it holds something the
//! shop is shown in the cart's currency — a saved cart reopened brings its
//! currency back with it. Changing currency with a cart in hand means
//! emptying it, which is asked, never assumed.

use topcoat::{
    context::Cx,
    cookie::{Cookie, Cookies, SameSite, cookies, time::Duration},
};

use crate::{cart_session::current_cart, db::shop_currencies};

/// Served as `__Host-timada_currency`. Not a secret and not a session: a
/// preference, kept a year.
const CURRENCY_COOKIE: &str = "timada_currency";
const CURRENCY_COOKIE_DAYS: i64 = 365;

fn jar(cx: &Cx) -> impl Cookies + '_ {
    cookies(cx)
        .override_same_site(SameSite::Lax)
        .override_http_only(true)
        .override_secure(true)
        .override_path("/")
        .override_prefix_host()
}

/// The currency the cookie asks for, when the shop sells in it; the base
/// currency otherwise.
fn chosen(cx: &Cx) -> String {
    let cookie = jar(cx).get(CURRENCY_COOKIE);
    shop_currencies()
        .or_base(cookie.as_ref().map(|c| c.value_trimmed()))
        .to_owned()
}

/// The currency of the cart being filled, when it holds anything.
pub async fn cart_currency(cx: &Cx) -> topcoat::Result<Option<String>> {
    match current_cart(cx).await {
        Ok(cart) => Ok(cart
            .as_ref()
            .and_then(|cart| cart.lines.first())
            .map(|line| line.unit_price.currency.clone())),
        Err(err) => Err(topcoat::Error::msg(format!("{err:#}"))),
    }
}

/// The currency prices are shown and charged in for this browser.
pub async fn shopper_currency(cx: &Cx) -> topcoat::Result<String> {
    Ok(match cart_currency(cx).await? {
        Some(currency) => currency,
        None => chosen(cx),
    })
}

/// Remembers the shopper's choice. `code` must be one the shop sells in.
pub fn choose_currency(cx: &Cx, code: &str) {
    jar(cx)
        .override_max_age(Duration::days(CURRENCY_COOKIE_DAYS))
        .add(Cookie::new(CURRENCY_COOKIE, code.to_owned()));
}
