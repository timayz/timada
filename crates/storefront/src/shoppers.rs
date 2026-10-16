//! Shopper accounts: a shop's customers who sign in with a mailed code.
//!
//! There is no sign-up: [`request_sign_in_code`] has the dashboard mail a
//! six-digit code from the shop, and [`sign_in`] trades it for a token, the
//! first time creating the account. The host keeps the token in its
//! shopper's browser (a private cookie, typically) and hands it back:
//! [`shopper`] reads who it signs in from the store, [`shopper_orders`] lists
//! every order placed with their address (guest ones included), and the
//! account keeps the addresses [`save_address`] gives it. A shopper may
//! [`sign_out_everywhere_else`] and [`delete_account`]. Codes and sessions
//! go through the dashboard over HTTP, as
//! [`StoreConfig::api_url`](crate::StoreConfig::api_url) says, so no key can
//! sign anyone in by writing to the store.

use std::net::IpAddr;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Context as _;
use serde::Deserialize;
use topcoat_core::context::Cx;
use topcoat_core::error::{Error, Result};
use topcoat_router::request::client_ip;

use timada_catalog::format_money;
use timada_orders::OrderStatus;
pub use timada_orders::ShippingAddress;
use timada_shoppers::SHOPPER_CODES;
pub use timada_shoppers::{MAX_ADDRESSES, SavedAddress, ShopperState};

use crate::context::store;
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
}

impl ShopperOrder {
    pub fn status(&self) -> OrderStatus {
        OrderStatus::parse(&self.status).unwrap_or_default()
    }

    /// The total as a person reads it (`24.00 EUR`).
    pub fn total_label(&self) -> String {
        format_money(self.total_cents.max(0) as u64, &self.currency)
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

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or_default()
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
    /// address counts for every shopper.
    pub async fn request_sign_in_code(
        &self,
        email: &str,
        ip: Option<IpAddr>,
    ) -> anyhow::Result<ShopperRefusable<()>> {
        let mut request = self
            .shopper_call(reqwest::Method::POST, "code", None)?
            .form(&[("email", email)]);
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

    /// Who `token` signs in, read from the store: `None` once it was signed
    /// out or expired.
    pub async fn shopper(&self, token: &str) -> anyhow::Result<Option<ShopperState>> {
        let resolved = timada_shoppers::Command(self.executor().clone())
            .resolve_session(token, now())
            .await?;
        Ok(resolved.map(|resolved| resolved.shopper))
    }

    /// Ends the session `token` opened.
    pub async fn sign_out(&self, token: &str) -> anyhow::Result<()> {
        let response = self
            .shopper_call(reqwest::Method::POST, "sign-out", Some(token))?
            .send()
            .await;
        answer(response, reqwest::StatusCode::NO_CONTENT, |_| Ok(()))
            .await?
            .ok();
        Ok(())
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
            let history: History =
                serde_json::from_slice(body).context("the dashboard's answer is not one")?;
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
    let refusable = matches!(status.as_u16(), 401 | 422 | 429);
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

/// Has the dashboard mail a sign-in code; see [`Store::request_sign_in_code`].
/// The shopper's address is the request's client, as the host's trusted
/// proxies report it.
pub async fn request_sign_in_code(cx: &Cx, email: &str) -> Result<ShopperRefusable<()>> {
    store(cx)
        .request_sign_in_code(email, client_ip(cx))
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
pub async fn shopper(cx: &Cx, token: &str) -> Result<Option<ShopperState>> {
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
