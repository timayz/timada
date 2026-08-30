//! The cart cookie: the only thing tying a browser to its cart.

use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};

/// Name of the cookie holding the cart's ULID.
pub const CART_COOKIE: &str = "timada_cart";

/// The cart id this browser carries, if any.
pub fn cart_cookie_id(jar: &CookieJar) -> Option<String> {
    jar.get(CART_COOKIE).map(|cookie| cookie.value().to_owned())
}

/// Build the cookie for `cart_id`.
///
/// `HttpOnly` because no script needs to read it, `SameSite=Lax` so the cart
/// survives a normal navigation back to the store but is not sent on
/// cross-site POSTs, and path `/` because the cart is read from the storefront
/// root, the cart page and the checkout alike. It is deliberately a session
/// cookie: a guest cart that outlives the browser session would resurrect
/// prices the catalog has since moved on from.
pub fn cart_cookie(cart_id: String) -> Cookie<'static> {
    Cookie::build((CART_COOKIE, cart_id))
        .path("/")
        .same_site(SameSite::Lax)
        .http_only(true)
        .build()
}
