//! Which cart a browser is filling: the cart id travels in a hardened cookie
//! so guests can shop before signing in. The cart itself is the `Cart`
//! aggregate; nothing about it is stored here.

use timada_cart::{CartDetailsView, CartStatus, load_cart_details};
use topcoat::{
    context::{Cx, app_context, memoize},
    cookie::{Cookie, Cookies, SameSite, cookies, time::Duration},
};

use crate::{Store, auth::current_account};

/// Served as `__Host-timada_cart`.
const CART_COOKIE: &str = "timada_cart";
const CART_COOKIE_DAYS: i64 = 30;

fn jar(cx: &Cx) -> impl Cookies + '_ {
    cookies(cx)
        .override_same_site(SameSite::Lax)
        .override_http_only(true)
        .override_secure(true)
        .override_path("/")
        .override_prefix_host()
}

/// The cart this browser is filling, if it is still editable and not
/// someone else's. Memoized per request.
#[memoize(as_ref)]
pub async fn current_cart(cx: &Cx) -> topcoat::Result<Option<CartDetailsView>> {
    let Some(cookie) = jar(cx).get(CART_COOKIE) else {
        return Ok(None);
    };
    let store = app_context::<Store>(cx);
    let Some(cart) = load_cart_details(&store.executor, cookie.value_trimmed()).await? else {
        return Ok(None);
    };
    // Checked out, parked in the saved carts, or deleted: not being filled.
    if cart.status != CartStatus::Open {
        return Ok(None);
    }
    // A cart opened by a signed-in shopper stays theirs.
    if let Some(owner) = &cart.customer_id {
        let account = current_account(cx)
            .await
            .map_err(|err| anyhow::anyhow!("{err:#}"))?;
        if account.as_ref().map(|a| &a.customer_id) != Some(owner) {
            return Ok(None);
        }
    }
    Ok(Some(cart))
}

/// The id of the current cart, opening one (and issuing the cookie) if needed.
pub async fn ensure_cart(cx: &Cx) -> topcoat::Result<String> {
    match current_cart(cx).await {
        Ok(Some(cart)) => return Ok(cart.id.clone()),
        Ok(None) => {}
        Err(err) => return Err(anyhow::anyhow!("{err:#}").into()),
    }
    let customer_id = match current_account(cx).await {
        Ok(account) => account.as_ref().map(|a| a.customer_id.clone()),
        Err(err) => return Err(anyhow::anyhow!("{err:#}").into()),
    };
    let store = app_context::<Store>(cx);
    let id = timada_cart::Command(&store.executor)
        .open_cart(customer_id)
        .await
        .map_err(anyhow::Error::from)?;
    jar(cx)
        .override_max_age(Duration::days(CART_COOKIE_DAYS))
        .add(Cookie::new(CART_COOKIE, id.clone()));
    Ok(id)
}

/// Makes `cart_id` the cart this browser is filling (a saved cart reopened).
pub fn use_cart(cx: &Cx, cart_id: &str) {
    jar(cx)
        .override_max_age(Duration::days(CART_COOKIE_DAYS))
        .add(Cookie::new(CART_COOKIE, cart_id.to_owned()));
}

/// Drops the cookie (after checkout, when the cart is saved for later, or
/// when the shopper signs out).
pub fn forget_cart(cx: &Cx) {
    jar(cx).remove(Cookie::new(CART_COOKIE, ""));
}
