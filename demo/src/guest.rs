//! Shoppers without an account. A guest is a customer like any other —
//! registered while ordering, with nothing to sign in to — so two things
//! stand in for the session they do not have:
//!
//! - a hardened cookie naming the guest customer this browser registered, so
//!   the checkout, the payment step and the order page know who is ordering;
//! - a **signed link** per order, written in the e-mails, that opens the
//!   order from anywhere.
//!
//! Both are HMACs of an id under the shop's link secret: nothing is stored,
//! and changing the secret calls every link and cookie off. Neither outlives
//! the guest: once they open an account, they sign in like anybody else.

use hmac::{Hmac, Mac};
use sha2::Sha256;
use timada_customer::load_address_book;
use topcoat::{
    context::{Cx, app_context, memoize},
    cookie::{Cookie, Cookies, SameSite, cookies, time::Duration},
};

use crate::{Store, auth::current_account};

/// Served as `__Host-timada_guest`.
const GUEST_COOKIE: &str = "timada_guest";
const GUEST_COOKIE_DAYS: i64 = 30;

/// What a key is for: a key of one kind is never valid as another.
const ORDER: &str = "order";
const GUEST: &str = "guest";

/// HMAC takes a key of any length: `None` does not happen.
fn mac(secret: &[u8], purpose: &str, id: &str) -> Option<Hmac<Sha256>> {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret).ok()?;
    mac.update(purpose.as_bytes());
    mac.update(b":");
    mac.update(id.as_bytes());
    Some(mac)
}

/// An empty key — which opens nothing — should one not be computable.
fn key(secret: &[u8], purpose: &str, id: &str) -> String {
    mac(secret, purpose, id)
        .map(|mac| hex::encode(mac.finalize().into_bytes()))
        .unwrap_or_default()
}

/// Compared in constant time.
fn key_is_valid(secret: &[u8], purpose: &str, id: &str, key: &str) -> bool {
    let (Ok(key), Some(mac)) = (hex::decode(key), mac(secret, purpose, id)) else {
        return false;
    };
    mac.verify_slice(&key).is_ok()
}

/// Where a guest reads an order, key included: what the e-mails link to.
pub fn order_path(secret: &[u8], order_id: &str) -> String {
    format!("/order/{order_id}?cle={}", key(secret, ORDER, order_id))
}

pub fn order_key_is_valid(secret: &[u8], order_id: &str, key: &str) -> bool {
    key_is_valid(secret, ORDER, order_id, key)
}

fn jar(cx: &Cx) -> impl Cookies + '_ {
    cookies(cx)
        .override_same_site(SameSite::Lax)
        .override_http_only(true)
        .override_secure(true)
        .override_path("/")
        .override_prefix_host()
}

/// This browser orders as that guest from now on.
pub fn remember_guest(cx: &Cx, customer_id: &str) {
    let store = app_context::<Store>(cx);
    let value = format!(
        "{customer_id}.{}",
        key(&store.link_secret, GUEST, customer_id)
    );
    jar(cx).add(
        Cookie::build((GUEST_COOKIE, value))
            .max_age(Duration::days(GUEST_COOKIE_DAYS))
            .build(),
    );
}

/// The guest customer this browser orders as, while they are still a guest.
#[memoize(as_ref)]
pub async fn current_guest(cx: &Cx) -> topcoat::Result<Option<String>> {
    let Some(cookie) = jar(cx).get(GUEST_COOKIE) else {
        return Ok(None);
    };
    let Some((customer_id, signature)) = cookie.value_trimmed().split_once('.') else {
        return Ok(None);
    };
    let store = app_context::<Store>(cx);
    if !key_is_valid(&store.link_secret, GUEST, customer_id, signature) {
        return Ok(None);
    }
    // With an account, they sign in: the cookie is worth nothing any more.
    let guest = load_address_book(&store.executor, customer_id)
        .await?
        .is_some_and(|customer| customer.guest);
    Ok(guest.then(|| customer_id.to_owned()))
}

/// Who is ordering: the signed-in account, or the guest of this browser.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shopper {
    pub customer_id: String,
    pub guest: bool,
}

/// An account comes first: signing in ends shopping as a guest.
pub async fn current_shopper(cx: &Cx) -> topcoat::Result<Option<Shopper>> {
    let account = current_account(cx)
        .await
        .map_err(|err| anyhow::anyhow!("{err:#}"))?;
    if let Some(account) = account.as_ref() {
        return Ok(Some(Shopper {
            customer_id: account.customer_id.clone(),
            guest: false,
        }));
    }
    let guest = current_guest(cx)
        .await
        .map_err(|err| anyhow::anyhow!("{err:#}"))?;
    Ok(guest.as_ref().map(|customer_id| Shopper {
        customer_id: customer_id.clone(),
        guest: true,
    }))
}

/// The shopper, or a redirect to the login page that comes back here — a
/// guest who lost their cookie has the link in their e-mails.
pub async fn require_shopper(cx: &Cx) -> topcoat::Result<Shopper> {
    match current_shopper(cx).await? {
        Some(shopper) => Ok(shopper),
        None => Err(crate::auth::require_account(cx)
            .await
            .err()
            .unwrap_or_else(|| anyhow::anyhow!("an account without a shopper").into())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_key_opens_its_own_order_and_nothing_else() {
        let secret = b"secret";
        let path = order_path(secret, "order-1");
        let key = path
            .split_once("?cle=")
            .map(|(_, key)| key)
            .unwrap_or_default();
        assert_eq!(key.len(), 64);
        assert!(order_key_is_valid(secret, "order-1", key));
        assert!(!order_key_is_valid(secret, "order-2", key));
        assert!(!order_key_is_valid(b"another secret", "order-1", key));
        assert!(!order_key_is_valid(secret, "order-1", "not hex"));
        assert!(!order_key_is_valid(secret, "order-1", ""));
        // An order's key is not a guest's, even for the same id.
        assert!(!key_is_valid(secret, GUEST, "order-1", key));
    }
}
