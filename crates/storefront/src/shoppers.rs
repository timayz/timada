//! Shopper accounts: a shop's customers who sign in with a mailed code.
//!
//! There is no sign-up: [`request_sign_in_code`] has the dashboard mail a
//! six-digit code from the shop, and [`sign_in`] trades it for a token, the
//! first time creating the account. The host keeps the token in its
//! shopper's browser (a private cookie, typically) and hands it back:
//! [`shopper`] asks the dashboard who it signs in, [`shopper_orders`] lists
//! every order placed with their address (guest ones included), and the
//! account keeps the addresses [`save_address`] gives it. A shopper may
//! [`sign_out_everywhere_else`] and [`delete_account`]. Everything goes
//! through the dashboard over HTTP, as
//! [`StoreConfig::api_url`](crate::StoreConfig::api_url) says: no key can
//! sign anyone in by writing to the store, and the store holds a shopper's
//! personal data only sealed under a key of theirs that the dashboard keeps
//! (and destroys with the account).

use std::net::IpAddr;

use anyhow::Context as _;
use serde::Deserialize;
use topcoat_core::context::Cx;
use topcoat_core::error::{Error, Result};
use topcoat_router::request::{client_ip, headers};

use timada_orders::OrderStatus;
pub use timada_orders::ShippingAddress;
use timada_shoppers::SHOPPER_CODES;
pub use timada_shoppers::{MAX_ADDRESSES, SavedAddress, Shopper};

use crate::context::store;
use crate::money::MoneyFormat;
use crate::store::Store;

/// What the dashboard answers for a token that no longer signs anyone in:
/// the host forgets it.
pub const SHOPPER_SIGNED_OUT: &str = "signed_out";

/// The header the shopper's address travels in to the dashboard.
const SHOPPER_IP: &str = "x-shopper-ip";

/// Why a shopper's request was refused, to show them: `wrong_code`,
/// `code_expired`, `too_many_attempts`, `code_recently_sent`,
/// `too_many_requests`, `invalid_email`, an address refusal (`invalid_country`, …),
/// `too_many_addresses`, or [`SHOPPER_SIGNED_OUT`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShopperRefusal(pub &'static str);

impl ShopperRefusal {
    pub fn code(&self) -> &'static str {
        self.0
    }

    pub fn is_signed_out(&self) -> bool {
        self.0 == SHOPPER_SIGNED_OUT
    }
}

/// Done, or refused.
pub type ShopperRefusable<T> = std::result::Result<T, ShopperRefusal>;

/// A sign-in that checked out: the token to keep, until `expires_at` (unix
/// seconds).
#[derive(Clone, PartialEq, Eq, Deserialize)]
pub struct ShopperSignIn {
    pub token: String,
    pub expires_at: u64,
}

impl std::fmt::Debug for ShopperSignIn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ShopperSignIn")
            .field("token", &"******")
            .field("expires_at", &self.expires_at)
            .finish()
    }
}

/// One order of a shopper's history.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ShopperOrder {
    pub id: String,
    pub reference: String,
    status: String,
    pub item_count: i64,
    pub total_cents: i64,
    pub currency: String,
    /// Unix seconds.
    pub placed_at: i64,
    /// How the store writes an amount ([`Store::money`]).
    #[serde(skip)]
    pub money: MoneyFormat,
}

impl ShopperOrder {
    pub fn status(&self) -> OrderStatus {
        OrderStatus::parse(&self.status).unwrap_or_default()
    }

    /// The total as a person reads it (`24.00 EUR`, or as the host writes
    /// prices).
    pub fn total_label(&self) -> String {
        self.money
            .format(self.total_cents.max(0) as u64, &self.currency)
    }
}

#[derive(Deserialize)]
struct Refused {
    error: Option<String>,
}

#[derive(Deserialize)]
struct Saved {
    id: String,
}

#[derive(Deserialize)]
struct History {
    orders: Vec<ShopperOrder>,
}

/// A shopper as the dashboard answers one.
#[derive(Deserialize)]
struct WireShopper {
    id: String,
    email: String,
    registered_at: u64,
    addresses: Vec<WireSavedAddress>,
}

#[derive(Deserialize)]
struct WireSavedAddress {
    id: String,
    saved_at: u64,
    address: WireAddress,
}

#[derive(Deserialize)]
struct WireAddress {
    name: String,
    line1: String,
    line2: Option<String>,
    postal_code: String,
    city: String,
    country: String,
    phone: Option<String>,
}

impl From<WireShopper> for Shopper {
    fn from(wire: WireShopper) -> Self {
        Self {
            id: wire.id,
            email: wire.email,
            registered_at: wire.registered_at,
            addresses: wire
                .addresses
                .into_iter()
                .map(|saved| SavedAddress {
                    id: saved.id,
                    saved_at: saved.saved_at,
                    address: ShippingAddress {
                        name: saved.address.name,
                        line1: saved.address.line1,
                        line2: saved.address.line2,
                        postal_code: saved.address.postal_code,
                        city: saved.address.city,
                        country: saved.address.country,
                        phone: saved.address.phone,
                    },
                })
                .collect(),
        }
    }
}

impl Store {
    /// Whether shoppers can sign in: the host gave the dashboard's address
    /// ([`StoreConfig::api_url`](crate::StoreConfig::api_url)).
    pub fn signs_shoppers_in(&self) -> bool {
        self.api().is_some()
    }

    /// Has the dashboard mail a sign-in code to `email`, from the shop.
    /// `ip` is the shopper's address, which the dashboard holds to its own
    /// limit of codes (`too_many_requests` past it); without it, this host's
    /// address counts for every shopper. `language` is the shopper's, as
    /// the browser states it (a tag or a whole `Accept-Language` value):
    /// the shop mails in it when it has that wording.
    pub async fn request_sign_in_code(
        &self,
        email: &str,
        ip: Option<IpAddr>,
        language: Option<&str>,
    ) -> anyhow::Result<ShopperRefusable<()>> {
        let mut form = vec![("email", email)];
        if let Some(language) = language {
            form.push(("language", language));
        }
        let mut request = self
            .shopper_call(reqwest::Method::POST, "code", None)?
            .form(&form);
        if let Some(ip) = ip {
            request = request.header(SHOPPER_IP, ip.to_string());
        }
        let response = request.send().await;
        answer(response, reqwest::StatusCode::ACCEPTED, |_| Ok(())).await
    }

    /// Trades the mailed code for a session token; the first time, this
    /// creates the account.
    pub async fn sign_in(
        &self,
        email: &str,
        code: &str,
    ) -> anyhow::Result<ShopperRefusable<ShopperSignIn>> {
        let response = self
            .shopper_call(reqwest::Method::POST, "session", None)?
            .form(&[("email", email), ("code", code)])
            .send()
            .await;
        answer(response, reqwest::StatusCode::CREATED, |body| {
            serde_json::from_slice(body).context("the dashboard's answer is not one")
        })
        .await
    }

    /// Who `token` signs in, as the dashboard answers: `None` once it was
    /// signed out, expired, or the account deleted, and when no dashboard
    /// address is set.
    pub async fn shopper(&self, token: &str) -> anyhow::Result<Option<Shopper>> {
        if !self.signs_shoppers_in() {
            return Ok(None);
        }
        let response = self
            .shopper_call(reqwest::Method::GET, "me", Some(token))?
            .send()
            .await;
        let answered = answer(response, reqwest::StatusCode::OK, |body| {
            let shopper: WireShopper =
                serde_json::from_slice(body).context("the dashboard's answer is not one")?;
            Ok(Shopper::from(shopper))
        })
        .await?;
        match answered {
            Ok(shopper) => Ok(Some(shopper)),
            Err(refusal) if refusal.is_signed_out() => Ok(None),
            Err(refusal) => anyhow::bail!("the dashboard refused a shopper: {}", refusal.code()),
        }
    }

    /// Ends the session `token` opened. A session ended already is nothing
    /// to report; any other refusal is an error, so a page that signs out
    /// does not forget a cookie the dashboard still honours.
    pub async fn sign_out(&self, token: &str) -> anyhow::Result<()> {
        let response = self
            .shopper_call(reqwest::Method::POST, "sign-out", Some(token))?
            .send()
            .await;
        match answer(response, reqwest::StatusCode::NO_CONTENT, |_| Ok(())).await? {
            Ok(()) => Ok(()),
            Err(refusal) if refusal.is_signed_out() => Ok(()),
            Err(refusal) => anyhow::bail!("the dashboard refused a sign-out: {}", refusal.code()),
        }
    }

    /// Ends every other session of the account `token` signs in; this one
    /// stays signed in.
    pub async fn sign_out_everywhere_else(
        &self,
        token: &str,
    ) -> anyhow::Result<ShopperRefusable<()>> {
        let response = self
            .shopper_call(reqwest::Method::POST, "sign-out-everywhere", Some(token))?
            .send()
            .await;
        answer(response, reqwest::StatusCode::NO_CONTENT, |_| Ok(())).await
    }

    /// Deletes the account `token` signs in: its addresses are forgotten and
    /// every session ends, this one included. The orders placed with its
    /// address stay with the shop; signing in again starts a new account.
    pub async fn delete_account(&self, token: &str) -> anyhow::Result<ShopperRefusable<()>> {
        let response = self
            .shopper_call(reqwest::Method::POST, "delete", Some(token))?
            .send()
            .await;
        answer(response, reqwest::StatusCode::NO_CONTENT, |_| Ok(())).await
    }

    /// Keeps an address on the account `token` signs in: its id. One already
    /// kept moves first.
    pub async fn save_address(
        &self,
        token: &str,
        address: &ShippingAddress,
    ) -> anyhow::Result<ShopperRefusable<String>> {
        let response = self
            .shopper_call(reqwest::Method::POST, "addresses", Some(token))?
            .form(&[
                ("name", address.name.as_str()),
                ("line1", &address.line1),
                ("line2", address.line2.as_deref().unwrap_or_default()),
                ("postal_code", &address.postal_code),
                ("city", &address.city),
                ("country", &address.country),
                ("phone", address.phone.as_deref().unwrap_or_default()),
            ])
            .send()
            .await;
        answer(response, reqwest::StatusCode::CREATED, |body| {
            let saved: Saved =
                serde_json::from_slice(body).context("the dashboard's answer is not one")?;
            Ok(saved.id)
        })
        .await
    }

    /// Forgets a saved address of the account `token` signs in.
    pub async fn remove_address(
        &self,
        token: &str,
        address_id: &str,
    ) -> anyhow::Result<ShopperRefusable<()>> {
        let response = self
            .shopper_call(reqwest::Method::POST, "addresses/remove", Some(token))?
            .form(&[("address_id", address_id)])
            .send()
            .await;
        answer(response, reqwest::StatusCode::NO_CONTENT, |_| Ok(())).await
    }

    /// The latest orders placed with the address of the account `token`
    /// signs in, newest first. Listed by the dashboard, so an order paid a
    /// moment ago may be missing for a second.
    pub async fn shopper_orders(
        &self,
        token: &str,
    ) -> anyhow::Result<ShopperRefusable<Vec<ShopperOrder>>> {
        let response = self
            .shopper_call(reqwest::Method::GET, "orders", Some(token))?
            .send()
            .await;
        answer(response, reqwest::StatusCode::OK, |body| {
            let mut history: History =
                serde_json::from_slice(body).context("the dashboard's answer is not one")?;
            for order in &mut history.orders {
                order.money = self.money().clone();
            }
            Ok(history.orders)
        })
        .await
    }

    fn shopper_call(
        &self,
        method: reqwest::Method,
        path: &str,
        token: Option<&str>,
    ) -> anyhow::Result<reqwest::RequestBuilder> {
        let api = self
            .api()
            .context("no dashboard address: set StoreConfig::api_url to sign shoppers in")?;
        let mut request = api
            .client
            .request(method, format!("{}/store/shoppers/{path}", api.url))
            .bearer_auth(&api.token);
        if let Some(token) = token {
            request = request.header("x-shopper-token", token);
        }
        Ok(request)
    }
}

/// `expected` and its body, a refusal to show, or the failure.
async fn answer<T>(
    response: reqwest::Result<reqwest::Response>,
    expected: reqwest::StatusCode,
    read: impl FnOnce(&[u8]) -> anyhow::Result<T>,
) -> anyhow::Result<ShopperRefusable<T>> {
    let response = response.context("cannot reach the dashboard")?;
    let status = response.status();
    let body = response
        .bytes()
        .await
        .context("cannot read the dashboard's answer")?;
    if status == expected {
        return read(&body).map(Ok);
    }
    // 503 is the shop not taking writes for the moment (`writes_paused`).
    let refusable = matches!(status.as_u16(), 401 | 422 | 429 | 503);
    let error = serde_json::from_slice::<Refused>(&body)
        .ok()
        .and_then(|refused| refused.error);
    if let (true, Some(error)) = (refusable, error) {
        let code = SHOPPER_CODES
            .iter()
            .chain(&[SHOPPER_SIGNED_OUT])
            .find(|code| **code == error)
            .with_context(|| format!("the dashboard refused a shopper: {error}"))?;
        return Ok(Err(ShopperRefusal(code)));
    }
    anyhow::bail!("the dashboard did not answer as expected: {status}")
}

/// The language the request's browser browses in, as the primary subtag
/// of its `Accept-Language` (`fr`); `None` when it says nothing readable.
/// For [`Checkout::language`](timada_orders::Checkout) and the sign-in
/// code, so the shop mails the shopper in their language.
pub fn request_language(cx: &Cx) -> Option<String> {
    headers(cx)
        .get("accept-language")?
        .to_str()
        .ok()
        .and_then(timada_core::primary_language)
}

/// The cookie [`TIME_ZONE_SCRIPT`] keeps the browser's time zone in.
pub const TIME_ZONE_COOKIE: &str = "time_zone";

/// Puts the browser's IANA time zone (`Europe/Paris`) in the
/// [`TIME_ZONE_COOKIE`] cookie, for [`request_time_zone`] to read on the
/// next request: a page includes it once, in a `<script>`. Without
/// JavaScript there is no cookie, and the shop's own zone stands in.
pub const TIME_ZONE_SCRIPT: &str = "(function(){try{var z=Intl.DateTimeFormat().resolvedOptions().timeZone;if(z){document.cookie='time_zone='+encodeURIComponent(z)+';path=/;max-age=31536000;samesite=lax'}}catch(e){}})();";

/// The browser's time zone, as [`TIME_ZONE_SCRIPT`] left it in its cookie;
/// `None` without the cookie or with one the zone database does not name.
/// For [`Checkout::time_zone`](timada_orders::Checkout) and the newsletter
/// sign-ups, so a scheduled campaign reaches the shopper at the hour it
/// names in their own day.
pub fn request_time_zone(cx: &Cx) -> Option<String> {
    headers(cx)
        .get_all("cookie")
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(';'))
        .find_map(|pair| {
            let (name, value) = pair.trim().split_once('=')?;
            (name == TIME_ZONE_COOKIE).then(|| value.replace("%2F", "/").replace("%2f", "/"))
        })
        .and_then(|value| timada_core::time_zone(&value))
}

/// Has the dashboard mail a sign-in code; see [`Store::request_sign_in_code`].
/// The shopper's address is the request's client, as the host's trusted
/// proxies report it, and their language the request's `Accept-Language`.
pub async fn request_sign_in_code(cx: &Cx, email: &str) -> Result<ShopperRefusable<()>> {
    store(cx)
        .request_sign_in_code(email, client_ip(cx), request_language(cx).as_deref())
        .await
        .map_err(Error::from_anyhow)
}

/// Trades a mailed code for a session token; see [`Store::sign_in`].
pub async fn sign_in(cx: &Cx, email: &str, code: &str) -> Result<ShopperRefusable<ShopperSignIn>> {
    store(cx)
        .sign_in(email, code)
        .await
        .map_err(Error::from_anyhow)
}

/// Who `token` signs in, if anyone still.
pub async fn shopper(cx: &Cx, token: &str) -> Result<Option<Shopper>> {
    store(cx).shopper(token).await.map_err(Error::from_anyhow)
}

/// Ends the session `token` opened.
pub async fn sign_out(cx: &Cx, token: &str) -> Result<()> {
    store(cx).sign_out(token).await.map_err(Error::from_anyhow)
}

/// Ends every other session; see [`Store::sign_out_everywhere_else`].
pub async fn sign_out_everywhere_else(cx: &Cx, token: &str) -> Result<ShopperRefusable<()>> {
    store(cx)
        .sign_out_everywhere_else(token)
        .await
        .map_err(Error::from_anyhow)
}

/// Deletes the account; see [`Store::delete_account`].
pub async fn delete_account(cx: &Cx, token: &str) -> Result<ShopperRefusable<()>> {
    store(cx)
        .delete_account(token)
        .await
        .map_err(Error::from_anyhow)
}

/// Keeps an address on the account; see [`Store::save_address`].
pub async fn save_address(
    cx: &Cx,
    token: &str,
    address: &ShippingAddress,
) -> Result<ShopperRefusable<String>> {
    store(cx)
        .save_address(token, address)
        .await
        .map_err(Error::from_anyhow)
}

/// Forgets a saved address; see [`Store::remove_address`].
pub async fn remove_address(
    cx: &Cx,
    token: &str,
    address_id: &str,
) -> Result<ShopperRefusable<()>> {
    store(cx)
        .remove_address(token, address_id)
        .await
        .map_err(Error::from_anyhow)
}

/// The orders of the account; see [`Store::shopper_orders`].
pub async fn shopper_orders(cx: &Cx, token: &str) -> Result<ShopperRefusable<Vec<ShopperOrder>>> {
    store(cx)
        .shopper_orders(token)
        .await
        .map_err(Error::from_anyhow)
}
