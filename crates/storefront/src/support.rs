//! The contact form: what a visitor writes to the shop.
//!
//! [`send_message`] hands a message to the dashboard over HTTP, as
//! [`StoreConfig::api_url`](crate::StoreConfig::api_url) says, with the
//! visitor's address (for the dashboard's limits) and the language their
//! browser browses in (so the merchant answers in it). The merchant reads
//! it in the back office; nothing comes back to the storefront. The library
//! keeps no copy: a message is the merchant's, not the shop's catalog.

use std::net::IpAddr;

use anyhow::Context as _;
use serde::Deserialize;
use topcoat_core::context::Cx;
use topcoat_core::error::{Error, Result};
use topcoat_router::request::client_ip;

use timada_support::SHOPPER_CODES;
pub use timada_support::{
    MAX_MESSAGE_LEN, MAX_NAME_LEN as MAX_MESSAGE_NAME_LEN, MIN_MESSAGE_LEN,
    SUPPORT_TOO_MANY_REQUESTS,
};

use crate::context::store;
use crate::shoppers::request_language;
use crate::store::Store;

/// The header the visitor's address travels in to the dashboard.
const SHOPPER_IP: &str = "x-shopper-ip";

/// What a visitor wrote in the contact form, as the host read it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MessageDraft {
    pub first_name: String,
    pub last_name: String,
    pub email: String,
    pub body: String,
}

/// Why the dashboard did not take a message: a code to word in the shop's
/// language (`message_invalid_name`, `message_invalid_email`,
/// `message_invalid_body`, `too_many_requests`, `writes_paused`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MessageRefusal(&'static str);

impl MessageRefusal {
    pub fn code(&self) -> &'static str {
        self.0
    }
}

/// A message taken, with its id, or refused.
pub type MessageRefusable<T> = std::result::Result<T, MessageRefusal>;

/// What the dashboard answers.
#[derive(Debug, Deserialize)]
struct Answer {
    id: Option<String>,
    error: Option<String>,
}

impl Store {
    /// Whether the contact form has somewhere to go: the host gave the
    /// dashboard's address ([`StoreConfig::api_url`](crate::StoreConfig::api_url)).
    pub fn takes_messages(&self) -> bool {
        self.api().is_some()
    }

    /// Hands a message to the dashboard: its id, or why it was refused.
    /// `ip` is the visitor's address, which the dashboard limits messages
    /// by; `language` a tag or a whole `Accept-Language` value. Fails when
    /// the store has no [`api_url`](crate::StoreConfig::api_url) or the
    /// dashboard cannot be reached; a key without `store:checkout` is
    /// refused there.
    pub async fn send_message(
        &self,
        draft: MessageDraft,
        ip: Option<IpAddr>,
        language: Option<&str>,
    ) -> anyhow::Result<MessageRefusable<String>> {
        let api = self
            .api()
            .context("no dashboard address: set StoreConfig::api_url to take messages")?;
        let mut form = vec![
            ("first_name", draft.first_name),
            ("last_name", draft.last_name),
            ("email", draft.email),
            ("body", draft.body),
        ];
        if let Some(language) = language {
            form.push(("language", language.to_owned()));
        }
        let mut request = api
            .client
            .post(format!("{}/store/contact", api.url))
            .bearer_auth(&api.token)
            .form(&form);
        if let Some(ip) = ip {
            request = request.header(SHOPPER_IP, ip.to_string());
        }
        let response = request.send().await.context("cannot reach the dashboard")?;
        let status = response.status();
        // 422: the message was refused; 429: too many from this visitor or
        // this storefront; 503: the shop is not taking writes for the
        // moment (`writes_paused`). All carry a code to show.
        if status.is_success() || matches!(status.as_u16(), 422 | 429 | 503) {
            let answer: Answer = response
                .json()
                .await
                .context("the dashboard's answer is not one")?;
            return match (answer.id, answer.error) {
                (Some(id), _) => Ok(Ok(id)),
                (None, Some(error)) => {
                    let code = SHOPPER_CODES
                        .iter()
                        .find(|code| **code == error)
                        .with_context(|| format!("the dashboard refused a message: {error}"))?;
                    Ok(Err(MessageRefusal(code)))
                }
                (None, None) => anyhow::bail!("the dashboard's answer is not one"),
            };
        }
        anyhow::bail!("the dashboard did not take the message: {status}")
    }
}

/// Hands the request's message to the dashboard; see
/// [`Store::send_message`]. The visitor's address is the request's client,
/// as the host's trusted proxies report it, and their language the
/// request's `Accept-Language`.
pub async fn send_message(cx: &Cx, draft: MessageDraft) -> Result<MessageRefusable<String>> {
    store(cx)
        .send_message(draft, client_ip(cx), request_language(cx).as_deref())
        .await
        .map_err(Error::from_anyhow)
}

/// Whether the contact form has somewhere to go; see
/// [`Store::takes_messages`].
pub fn takes_messages(cx: &Cx) -> bool {
    store(cx).takes_messages()
}
