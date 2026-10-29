//! The shop's newsletter: shoppers ask for its news, confirm their address
//! through the link the shop mails, and leave again through the link of any
//! mail.
//!
//! Each call goes to the dashboard over HTTP, as
//! [`StoreConfig::api_url`](crate::StoreConfig::api_url) says. The links the
//! shop mails open pages of the host's storefront —
//! `{storefront}/newsletter/confirm/{token}` and
//! `{storefront}/newsletter/unsubscribe/{token}` — which hand the token back
//! through [`confirm_subscription`] and [`unsubscribe`]. The library keeps
//! no copy of the list: it is the merchant's.

use std::net::IpAddr;

use anyhow::Context as _;
use serde::Deserialize;
use topcoat_core::context::Cx;
use topcoat_core::error::{Error, Result};
use topcoat_router::request::client_ip;

use timada_newsletter::SHOPPER_CODES;
pub use timada_newsletter::{
    NEWSLETTER_INVALID_EMAIL, NEWSLETTER_INVALID_LINK, NEWSLETTER_LINK_EXPIRED,
    NEWSLETTER_TOO_MANY_REQUESTS, NEWSLETTER_UNAVAILABLE,
};

use crate::context::store;
use crate::shoppers::{SHOPPER_SIGNED_OUT, request_language, request_time_zone};
use crate::store::Store;

/// The header the visitor's address travels in to the dashboard.
const SHOPPER_IP: &str = "x-shopper-ip";

/// Why the dashboard did not do as asked: a code to word in the shop's
/// language (`newsletter_invalid_email`, `newsletter_invalid_link`,
/// `newsletter_link_expired`, `newsletter_unavailable`, `too_many_requests`,
/// `writes_paused`, and `signed_out` for a shopper's own switch).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NewsletterRefusal(&'static str);

impl NewsletterRefusal {
    pub fn code(&self) -> &'static str {
        self.0
    }

    /// The shopper's token no longer signs anyone in.
    pub fn is_signed_out(&self) -> bool {
        self.0 == SHOPPER_SIGNED_OUT
    }
}

/// Done, or refused.
pub type NewsletterRefusable<T> = std::result::Result<T, NewsletterRefusal>;

/// What a followed confirmation link did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Confirmation {
    Confirmed,
    /// The link was followed before: nothing changed.
    AlreadyConfirmed,
}

#[derive(Deserialize)]
struct Refused {
    error: Option<String>,
}

#[derive(Deserialize)]
struct Confirmed {
    status: String,
}

#[derive(Deserialize)]
struct Subscribed {
    subscribed: bool,
}

impl Store {
    /// Whether the newsletter has somewhere to go: the host gave the
    /// dashboard's address ([`StoreConfig::api_url`](crate::StoreConfig::api_url)).
    pub fn takes_newsletter_signups(&self) -> bool {
        self.api().is_some()
    }

    /// Hands in an address that asks for the shop's news: the shop mails it
    /// a link to confirm. The dashboard answers the same whether the
    /// address was mailed, is on the list already or was mailed a moment
    /// ago. `ip` is the visitor's address, which the dashboard limits
    /// sign-ups by; `language` a tag or a whole `Accept-Language` value;
    /// `time_zone` the browser's IANA zone.
    pub async fn subscribe(
        &self,
        email: &str,
        ip: Option<IpAddr>,
        language: Option<&str>,
        time_zone: Option<&str>,
    ) -> anyhow::Result<NewsletterRefusable<()>> {
        let mut form = vec![("email", email)];
        if let Some(language) = language {
            form.push(("language", language));
        }
        if let Some(time_zone) = time_zone {
            form.push(("time_zone", time_zone));
        }
        let mut request = self.newsletter_call(reqwest::Method::POST, "")?.form(&form);
        if let Some(ip) = ip {
            request = request.header(SHOPPER_IP, ip.to_string());
        }
        answer(request.send().await, reqwest::StatusCode::ACCEPTED, |_| {
            Ok(())
        })
        .await
    }

    /// Confirms the subscription a mailed link's `token` names.
    pub async fn confirm_subscription(
        &self,
        token: &str,
    ) -> anyhow::Result<NewsletterRefusable<Confirmation>> {
        let response = self
            .newsletter_call(reqwest::Method::POST, "/confirm")?
            .form(&[("token", token)])
            .send()
            .await;
        answer(response, reqwest::StatusCode::OK, |body| {
            let confirmed: Confirmed =
                serde_json::from_slice(body).context("the dashboard's answer is not one")?;
            Ok(match confirmed.status.as_str() {
                "already_confirmed" => Confirmation::AlreadyConfirmed,
                _ => Confirmation::Confirmed,
            })
        })
        .await
    }

    /// Takes the subscriber a mail's unsubscribe link names off the list;
    /// off it already is fine.
    pub async fn unsubscribe(&self, token: &str) -> anyhow::Result<NewsletterRefusable<()>> {
        let response = self
            .newsletter_call(reqwest::Method::POST, "/unsubscribe")?
            .form(&[("token", token)])
            .send()
            .await;
        answer(response, reqwest::StatusCode::NO_CONTENT, |_| Ok(())).await
    }

    /// Whether the shopper `token` signs in gets the shop's news.
    pub async fn shopper_newsletter(
        &self,
        token: &str,
    ) -> anyhow::Result<NewsletterRefusable<bool>> {
        let response = self
            .shopper_newsletter_call(reqwest::Method::GET, token)?
            .send()
            .await;
        answer(response, reqwest::StatusCode::OK, |body| {
            let subscribed: Subscribed =
                serde_json::from_slice(body).context("the dashboard's answer is not one")?;
            Ok(subscribed.subscribed)
        })
        .await
    }

    /// Puts the shopper `token` signs in on the list, or takes them off it.
    /// Their address is proved by their sign-in: no link to follow.
    pub async fn set_shopper_newsletter(
        &self,
        token: &str,
        subscribed: bool,
        language: Option<&str>,
        time_zone: Option<&str>,
    ) -> anyhow::Result<NewsletterRefusable<()>> {
        let mut form = vec![("subscribed", subscribed.to_string())];
        if let Some(language) = language {
            form.push(("language", language.to_owned()));
        }
        if let Some(time_zone) = time_zone {
            form.push(("time_zone", time_zone.to_owned()));
        }
        let response = self
            .shopper_newsletter_call(reqwest::Method::POST, token)?
            .form(&form)
            .send()
            .await;
        answer(response, reqwest::StatusCode::NO_CONTENT, |_| Ok(())).await
    }

    fn newsletter_call(
        &self,
        method: reqwest::Method,
        path: &str,
    ) -> anyhow::Result<reqwest::RequestBuilder> {
        let api = self.api().context(
            "no dashboard address: set StoreConfig::api_url to take newsletter sign-ups",
        )?;
        Ok(api
            .client
            .request(method, format!("{}/store/newsletter{path}", api.url))
            .bearer_auth(&api.token))
    }

    fn shopper_newsletter_call(
        &self,
        method: reqwest::Method,
        token: &str,
    ) -> anyhow::Result<reqwest::RequestBuilder> {
        let api = self
            .api()
            .context("no dashboard address: set StoreConfig::api_url to sign shoppers in")?;
        Ok(api
            .client
            .request(method, format!("{}/store/shoppers/newsletter", api.url))
            .bearer_auth(&api.token)
            .header("x-shopper-token", token))
    }
}

/// `expected` and its body, a refusal to show, or the failure.
async fn answer<T>(
    response: reqwest::Result<reqwest::Response>,
    expected: reqwest::StatusCode,
    read: impl FnOnce(&[u8]) -> anyhow::Result<T>,
) -> anyhow::Result<NewsletterRefusable<T>> {
    let response = response.context("cannot reach the dashboard")?;
    let status = response.status();
    let body = response
        .bytes()
        .await
        .context("cannot read the dashboard's answer")?;
    if status == expected {
        return read(&body).map(Ok);
    }
    // 401: signed out; 422: refused; 429: too many from this visitor or this
    // storefront; 503: the shop is not taking writes, or has no storefront
    // address to put in a link. All carry a code to show.
    let refusable = matches!(status.as_u16(), 401 | 422 | 429 | 503);
    let error = serde_json::from_slice::<Refused>(&body)
        .ok()
        .and_then(|refused| refused.error);
    if let (true, Some(error)) = (refusable, error) {
        let code = SHOPPER_CODES
            .iter()
            .chain(&[SHOPPER_SIGNED_OUT])
            .find(|code| **code == error)
            .with_context(|| format!("the dashboard refused a newsletter call: {error}"))?;
        return Ok(Err(NewsletterRefusal(code)));
    }
    anyhow::bail!("the dashboard did not answer as expected: {status}")
}

/// Hands in the request's address for the shop's news; see
/// [`Store::subscribe`]. The visitor's address is the request's client,
/// as the host's trusted proxies report it, their language the request's
/// `Accept-Language`, and their time zone [`request_time_zone`]'s.
pub async fn subscribe(cx: &Cx, email: &str) -> Result<NewsletterRefusable<()>> {
    store(cx)
        .subscribe(
            email,
            client_ip(cx),
            request_language(cx).as_deref(),
            request_time_zone(cx).as_deref(),
        )
        .await
        .map_err(Error::from_anyhow)
}

/// See [`Store::confirm_subscription`].
pub async fn confirm_subscription(
    cx: &Cx,
    token: &str,
) -> Result<NewsletterRefusable<Confirmation>> {
    store(cx)
        .confirm_subscription(token)
        .await
        .map_err(Error::from_anyhow)
}

/// See [`Store::unsubscribe`].
pub async fn unsubscribe(cx: &Cx, token: &str) -> Result<NewsletterRefusable<()>> {
    store(cx)
        .unsubscribe(token)
        .await
        .map_err(Error::from_anyhow)
}

/// See [`Store::shopper_newsletter`].
pub async fn shopper_newsletter(cx: &Cx, token: &str) -> Result<NewsletterRefusable<bool>> {
    store(cx)
        .shopper_newsletter(token)
        .await
        .map_err(Error::from_anyhow)
}

/// See [`Store::set_shopper_newsletter`]; the language and time zone are
/// the request's.
pub async fn set_shopper_newsletter(
    cx: &Cx,
    token: &str,
    subscribed: bool,
) -> Result<NewsletterRefusable<()>> {
    store(cx)
        .set_shopper_newsletter(
            token,
            subscribed,
            request_language(cx).as_deref(),
            request_time_zone(cx).as_deref(),
        )
        .await
        .map_err(Error::from_anyhow)
}

/// Whether the newsletter has somewhere to go; see
/// [`Store::takes_newsletter_signups`].
pub fn takes_newsletter_signups(cx: &Cx) -> bool {
    store(cx).takes_newsletter_signups()
}
