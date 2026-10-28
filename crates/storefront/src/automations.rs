//! The shop's automated mails, from the storefront's side: every one ends
//! with a link (`{storefront}/emails/opt-out/{token}`) whose page hands the
//! token back here, and the address stops getting them. Orders are still
//! confirmed, and the newsletter is a list of its own.

use anyhow::Context as _;
use topcoat_core::context::Cx;
use topcoat_core::error::{Error, Result};

use crate::context::store;
use crate::store::Store;

impl Store {
    /// Stops the shop's automated mails to the address a link names:
    /// whether the link checked out (`false`: cut short, or another shop's).
    /// Stopped already is fine.
    pub async fn stop_automated_mails(&self, token: &str) -> anyhow::Result<bool> {
        let api = self
            .api()
            .context("no dashboard address: set StoreConfig::api_url to stop automated mails")?;
        let response = api
            .client
            .post(format!("{}/store/automations/opt-out", api.url))
            .bearer_auth(&api.token)
            .form(&[("token", token)])
            .send()
            .await
            .context("cannot reach the dashboard")?;
        match response.status().as_u16() {
            204 => Ok(true),
            422 => Ok(false),
            status => anyhow::bail!("the dashboard did not answer as expected: {status}"),
        }
    }
}

/// See [`Store::stop_automated_mails`].
pub async fn stop_automated_mails(cx: &Cx, token: &str) -> Result<bool> {
    store(cx)
        .stop_automated_mails(token)
        .await
        .map_err(Error::from_anyhow)
}
