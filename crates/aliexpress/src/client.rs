//! The calls to AliExpress's open platform, signed the way it wants them:
//! every parameter, sorted by name and run together as `name` + `value`
//! (after the API path, for the calls addressed by path), HMAC-SHA256 under
//! the app's secret, in upper-case hex. Dotted methods
//! (`aliexpress.ds.product.get`) go to `<api>/sync` and name themselves in
//! `method`; path methods (`/auth/token/create`) go to `<api>/rest<path>`.

use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

use hmac::{Hmac, Mac};
use serde_json::Value;
use sha2::Sha256;

use crate::model::{AeProduct, Market, Tokens};

/// AliExpress's own gateway.
pub const DEFAULT_API_URL: &str = "https://api-sg.aliexpress.com";
/// Where an AliExpress account says yes to an app.
pub const DEFAULT_AUTHORIZE_URL: &str = "https://api-sg.aliexpress.com/oauth/authorize";

/// Why a call came back without what it asked for.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("AliExpress refused ({code}): {message}")]
pub struct AeError {
    /// AliExpress's own code, or `http_<status>` / `network` / `response`.
    pub code: String,
    pub message: String,
    /// Worth trying again later (the network, a limit, AliExpress itself).
    pub retryable: bool,
    /// The item is no longer sold (removed, or never was).
    pub gone: bool,
}

impl AeError {
    fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        let code = code.into();
        let message = message.into();
        let lowered = format!("{code} {message}").to_ascii_lowercase();
        let retryable = lowered.contains("limit")
            || lowered.contains("frequency")
            || lowered.contains("timeout")
            || lowered.contains("isp.")
            || lowered.contains("system error");
        let gone = lowered.contains("not exist")
            || lowered.contains("not found")
            || lowered.contains("offline")
            || lowered.contains("off shelf")
            || lowered.contains("off-shelf");
        Self {
            code,
            message,
            retryable,
            gone,
        }
    }

    fn transient(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            retryable: true,
            gone: false,
        }
    }
}

/// An app's client: its key and secret, and the token of the account it
/// acts for once one said yes.
#[derive(Clone)]
pub struct Client {
    http: reqwest::Client,
    api_url: String,
    app_key: String,
    app_secret: String,
    session: Option<String>,
}

impl std::fmt::Debug for Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Client")
            .field("api_url", &self.api_url)
            .field("app_key", &self.app_key)
            .finish_non_exhaustive()
    }
}

/// The signature of a call: `path` for the calls addressed by path,
/// `params` every parameter but `sign`.
pub fn sign(app_secret: &str, path: Option<&str>, params: &BTreeMap<String, String>) -> String {
    let mut base = String::from(path.unwrap_or_default());
    for (name, value) in params {
        base.push_str(name);
        base.push_str(value);
    }
    let mut mac =
        Hmac::<Sha256>::new_from_slice(app_secret.as_bytes()).expect("HMAC takes any key length");
    mac.update(base.as_bytes());
    mac.finalize()
        .into_bytes()
        .iter()
        .map(|byte| format!("{byte:02X}"))
        .collect()
}

/// Where to send the merchant so their AliExpress account says yes to the
/// app; AliExpress sends them back to `redirect_uri` with `code` and
/// `state`.
pub fn authorize_url(base: &str, app_key: &str, redirect_uri: &str, state: &str) -> String {
    let mut url = reqwest::Url::parse(base).unwrap_or_else(|_| {
        reqwest::Url::parse(DEFAULT_AUTHORIZE_URL).expect("the default address parses")
    });
    url.query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("force_auth", "true")
        .append_pair("client_id", app_key)
        .append_pair("redirect_uri", redirect_uri)
        .append_pair("state", state);
    url.to_string()
}

impl AeProduct {
    /// The product in the answer to `aliexpress.ds.product.get`, or why
    /// there is none.
    pub fn from_response(product_id: &str, body: &Value) -> Result<Self, AeError> {
        let response = body
            .get("aliexpress_ds_product_get_response")
            .unwrap_or(body);
        let code = response
            .get("rsp_code")
            .map(crate::model::text)
            .unwrap_or_default();
        if !code.is_empty() && code != "200" {
            let message = response
                .get("rsp_msg")
                .map(crate::model::text)
                .unwrap_or_default();
            return Err(AeError::new(code, message));
        }
        let result = response
            .get("result")
            .ok_or_else(|| AeError::new("response", "no product in the answer"))?;
        AeProduct::from_json(product_id, result)
            .ok_or_else(|| AeError::new("response", "the product could not be read"))
    }
}

impl Client {
    pub fn new(
        http: reqwest::Client,
        api_url: &str,
        app_key: impl Into<String>,
        app_secret: impl Into<String>,
    ) -> Self {
        Self {
            http,
            api_url: api_url.trim_end_matches('/').to_owned(),
            app_key: app_key.into(),
            app_secret: app_secret.into(),
            session: None,
        }
    }

    /// The same client acting for the account `access_token` stands for.
    pub fn with_session(mut self, access_token: impl Into<String>) -> Self {
        self.session = Some(access_token.into());
        self
    }

    pub fn app_key(&self) -> &str {
        &self.app_key
    }

    /// Exchanges the code AliExpress sent the merchant back with for the
    /// account's tokens.
    pub async fn create_token(&self, code: &str) -> Result<Tokens, AeError> {
        let body = self
            .call("/auth/token/create", vec![("code", code.to_owned())])
            .await?;
        Tokens::from_json(&body).ok_or_else(|| AeError::new("response", "no token in the answer"))
    }

    /// New tokens for the account, before the access token lapses.
    pub async fn refresh_token(&self, refresh_token: &str) -> Result<Tokens, AeError> {
        let body = self
            .call(
                "/auth/token/refresh",
                vec![("refresh_token", refresh_token.to_owned())],
            )
            .await?;
        Tokens::from_json(&body).ok_or_else(|| AeError::new("response", "no token in the answer"))
    }

    /// One product as a dropshipper buys it: shipped to the market's
    /// country, priced in its currency and worded in its language.
    pub async fn product(&self, product_id: &str, market: &Market) -> Result<AeProduct, AeError> {
        let body = self
            .call(
                "aliexpress.ds.product.get",
                vec![
                    ("product_id", product_id.to_owned()),
                    ("ship_to_country", market.ship_to_country.clone()),
                    ("target_currency", market.currency.clone()),
                    ("target_language", market.language.to_ascii_uppercase()),
                ],
            )
            .await?;
        AeProduct::from_response(product_id, &body)
    }

    /// Signs and sends one call; the answer's body, or why there is none.
    async fn call(&self, method: &str, params: Vec<(&str, String)>) -> Result<Value, AeError> {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|elapsed| elapsed.as_millis())
            .unwrap_or_default();
        let mut all: BTreeMap<String, String> = params
            .into_iter()
            .filter(|(_, value)| !value.is_empty())
            .map(|(name, value)| (name.to_owned(), value))
            .collect();
        all.insert("app_key".into(), self.app_key.clone());
        all.insert("sign_method".into(), "sha256".into());
        all.insert("timestamp".into(), timestamp.to_string());
        all.insert("simplify".into(), "true".into());
        if let Some(session) = &self.session {
            all.insert("session".into(), session.clone());
        }
        let (url, path) = if method.starts_with('/') {
            (format!("{}/rest{method}", self.api_url), Some(method))
        } else {
            all.insert("method".into(), method.to_owned());
            (format!("{}/sync", self.api_url), None)
        };
        let signature = sign(&self.app_secret, path, &all);
        all.insert("sign".into(), signature);
        let response = self
            .http
            .post(&url)
            .query(&all)
            .send()
            .await
            .map_err(|err| AeError::transient("network", err.to_string()))?;
        let status = response.status();
        if status.is_server_error() || status.as_u16() == 429 {
            return Err(AeError::transient(
                format!("http_{}", status.as_u16()),
                status.to_string(),
            ));
        }
        if !status.is_success() {
            return Err(AeError::new(
                format!("http_{}", status.as_u16()),
                status.to_string(),
            ));
        }
        let body: Value = response
            .json()
            .await
            .map_err(|err| AeError::transient("response", err.to_string()))?;
        if let Some(error) = body.get("error_response") {
            let code = error
                .get("code")
                .map(crate::model::text)
                .unwrap_or_default();
            let sub = error
                .get("sub_code")
                .map(crate::model::text)
                .unwrap_or_default();
            let message = error
                .get("msg")
                .or_else(|| error.get("sub_msg"))
                .map(crate::model::text)
                .unwrap_or_default();
            let code = if sub.is_empty() { code } else { sub };
            return Err(AeError::new(code, message));
        }
        // The calls addressed by path answer with a `code` of their own.
        if path.is_some()
            && let Some(code) = body.get("code").map(crate::model::text)
            && code != "0"
            && !code.is_empty()
        {
            let message = body
                .get("message")
                .or_else(|| body.get("msg"))
                .map(crate::model::text)
                .unwrap_or_default();
            return Err(AeError::new(code, message));
        }
        Ok(body)
    }
}
