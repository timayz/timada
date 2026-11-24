//! Google, as the pages and the passes reach it: the application the shops
//! connect their accounts to, and what is read through a shop's grant.
//!
//! A merchant is sent to Google with the kinds of access the shop turns on
//! ([`Google::authorize_url`]); Google sends them back with a code, which
//! is exchanged for a grant ([`Google::exchange`]). The grant's refresh
//! token is sealed for the shop and recorded on its connection
//! (`timada-google`); it does not lapse while it is used. The access it
//! renews lasts an hour, so it is never recorded: each node keeps the one
//! it last got, by shop, and asks for another a few minutes before that one
//! lapses ([`Google::access`]). Recording each would write a shop's stream
//! twenty-four times a day for a value any node can ask for again.
//!
//! Google refusing to renew a grant (`invalid_grant`: the account withdrew
//! it, or it went unused for six months) is recorded on the connection as
//! lapsed, once, by whichever page or pass met it: [`GoogleFault::ConnectAgain`].
//!
//! Taking a connection off forgets the grant here and does not withdraw it
//! at Google: Google withdraws per account and application, not per grant,
//! so withdrawing one shop's would end every other shop's connected to the
//! same account. The page says where the account withdraws it.

mod search_console;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde::de::DeserializeOwned;
use timada_google::{ConnectionState, GOOGLE_ACTOR, accesses_of, scope_of};

use crate::config::GoogleConfig;
use crate::seal::Sealer;

pub use search_console::Site;

/// Where a merchant says yes.
pub const GOOGLE_AUTHORIZE_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";
/// Where a code is exchanged for a grant, and a grant renewed.
pub const GOOGLE_TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
/// Where the account behind a grant is named.
pub const GOOGLE_USERINFO_URL: &str = "https://openidconnect.googleapis.com/v1/userinfo";
/// Where Search Console is read.
pub const SEARCH_CONSOLE_URL: &str = "https://www.googleapis.com/webmasters/v3";

/// How long Google is given to answer.
pub const GOOGLE_TIMEOUT: Duration = Duration::from_secs(20);

/// How long before an access lapses another is asked for.
const RENEW_BEFORE: Duration = Duration::from_secs(5 * 60);

/// The kinds of access a shop is offered to turn on: the ones something
/// reads through. Google Ads and Merchant Center join the list with what
/// reads them.
pub const OFFERED: &[&str] = &[timada_google::ACCESS_SEARCH_CONSOLE];

/// Why Google gave nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GoogleFault {
    /// The operator set no application: no shop connects.
    Off,
    /// Google no longer honours the shop's grant: the merchant has to
    /// connect again.
    ConnectAgain,
    /// Google could not answer (no answer in time, its own failure, asked
    /// too often): asking again later may do.
    Unavailable(String),
    /// Google will say the same again, in its words.
    Refused(String),
}

impl std::fmt::Display for GoogleFault {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Off => f.write_str("no Google application is set"),
            Self::ConnectAgain => f.write_str("Google no longer honours the connection"),
            Self::Unavailable(reason) => write!(f, "Google is unavailable: {reason}"),
            Self::Refused(reason) => write!(f, "Google refused: {reason}"),
        }
    }
}

impl std::error::Error for GoogleFault {}

/// What Google hands back for a code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Grant {
    /// What renews the access; `None` when Google gave none, which it does
    /// only for an account that was not asked to say yes again.
    pub refresh_token: Option<String>,
    pub access_token: String,
    /// For how many seconds the access is good.
    pub expires_in: u64,
    /// The kinds of access the account granted: it may untick some.
    pub accesses: Vec<String>,
}

/// An access a node holds for a shop, and the grant it was renewed from.
struct Held {
    refresh_token_sealed: Vec<u8>,
    access_token: String,
    until: Instant,
}

struct Inner {
    client_id: String,
    client_secret: String,
    authorize_url: String,
    token_url: String,
    userinfo_url: String,
    search_console_url: String,
    sealer: Sealer,
    client: reqwest::Client,
    held: Mutex<HashMap<String, Held>>,
}

/// Google as the pages and the passes reach it; nothing inside when the
/// operator set no application, or no key to seal the grants under.
#[derive(Clone)]
pub struct Google(Option<Arc<Inner>>);

impl std::fmt::Debug for Google {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Google")
            .field("enabled", &self.enabled())
            .finish()
    }
}

#[derive(Deserialize)]
struct TokenAnswer {
    #[serde(default)]
    access_token: String,
    #[serde(default)]
    expires_in: u64,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    scope: String,
}

#[derive(Deserialize, Default)]
struct TokenRefusal {
    #[serde(default)]
    error: String,
    #[serde(default)]
    error_description: String,
}

#[derive(Deserialize)]
struct UserInfo {
    #[serde(default)]
    email: String,
}

/// What Google's APIs answer a refused call with.
#[derive(Deserialize, Default)]
struct ApiRefusal {
    #[serde(default)]
    error: ApiError,
}

#[derive(Deserialize, Default)]
struct ApiError {
    #[serde(default)]
    message: String,
}

/// A reason in Google's words, short enough to keep and to show.
fn reason(text: &str) -> String {
    text.trim().chars().take(300).collect()
}

fn unavailable(err: reqwest::Error) -> GoogleFault {
    // The error names the address, never a secret sent to it.
    GoogleFault::Unavailable(reason(&err.without_url().to_string()))
}

/// Opens what the token address answered.
fn open_token(status: u16, body: &[u8]) -> Result<TokenAnswer, GoogleFault> {
    if status == 200 {
        let answer: TokenAnswer = serde_json::from_slice(body)
            .map_err(|err| GoogleFault::Unavailable(format!("unreadable answer: {err}")))?;
        if answer.access_token.is_empty() {
            return Err(GoogleFault::Unavailable("an answer with no access".into()));
        }
        return Ok(answer);
    }
    let refusal: TokenRefusal = serde_json::from_slice(body).unwrap_or_default();
    if refusal.error == "invalid_grant" {
        return Err(GoogleFault::ConnectAgain);
    }
    let said = if refusal.error_description.is_empty() {
        format!("{} (HTTP {status})", refusal.error)
    } else {
        refusal.error_description
    };
    if status == 429 || status >= 500 {
        Err(GoogleFault::Unavailable(reason(&said)))
    } else {
        Err(GoogleFault::Refused(reason(&said)))
    }
}

/// Opens what one of Google's APIs answered a call made under an access.
fn open_api<T: DeserializeOwned>(status: u16, body: &[u8]) -> Result<T, GoogleFault> {
    if status == 200 {
        return serde_json::from_slice(body)
            .map_err(|err| GoogleFault::Unavailable(format!("unreadable answer: {err}")));
    }
    let refusal: ApiRefusal = serde_json::from_slice(body).unwrap_or_default();
    let said = if refusal.error.message.is_empty() {
        format!("HTTP {status}")
    } else {
        refusal.error.message
    };
    match status {
        // The access is turned down: [`Google::read`] asks for another.
        401 => Err(GoogleFault::ConnectAgain),
        429 | 500.. => Err(GoogleFault::Unavailable(reason(&said))),
        _ => Err(GoogleFault::Refused(reason(&said))),
    }
}

impl Google {
    /// The application the configuration sets; off without its id and
    /// secret, or without `sealer` (no `secrets.key`).
    pub fn new(sealer: Option<Sealer>, config: &GoogleConfig) -> Self {
        let Some(((client_id, client_secret), sealer)) = config.credentials().zip(sealer) else {
            return Self(None);
        };
        let address = |set: &Option<String>, own: &str| {
            set.as_deref()
                .filter(|url| !url.is_empty())
                .unwrap_or(own)
                .trim_end_matches('/')
                .to_owned()
        };
        Self(Some(Arc::new(Inner {
            client_id: client_id.to_owned(),
            client_secret: client_secret.to_owned(),
            authorize_url: address(&config.authorize_url, GOOGLE_AUTHORIZE_URL),
            token_url: address(&config.token_url, GOOGLE_TOKEN_URL),
            userinfo_url: address(&config.userinfo_url, GOOGLE_USERINFO_URL),
            search_console_url: address(&config.search_console_url, SEARCH_CONSOLE_URL),
            sealer,
            client: reqwest::Client::builder()
                .timeout(GOOGLE_TIMEOUT)
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .unwrap_or_default(),
            held: Mutex::default(),
        })))
    }

    /// Whether a shop can connect an account.
    pub fn enabled(&self) -> bool {
        self.0.is_some()
    }

    fn inner(&self) -> Result<&Inner, GoogleFault> {
        self.0.as_deref().ok_or(GoogleFault::Off)
    }

    /// Where to send a merchant so their account says yes to `accesses`,
    /// and comes back to `redirect_uri` with `state`. The account is asked
    /// every time (`prompt=consent`), which is what makes Google hand a
    /// refresh token back, and what it granted before stays granted
    /// (`include_granted_scopes`). Its e-mail address is asked for too, to
    /// name it on the page.
    pub fn authorize_url(
        &self,
        redirect_uri: &str,
        state: &str,
        accesses: &[String],
    ) -> anyhow::Result<String> {
        let inner = self.inner()?;
        let mut scopes = vec!["openid", "email"];
        for access in accesses {
            scopes
                .push(scope_of(access).ok_or_else(|| anyhow::anyhow!("no such access: {access}"))?);
        }
        let url = reqwest::Url::parse_with_params(
            &inner.authorize_url,
            [
                ("client_id", inner.client_id.as_str()),
                ("redirect_uri", redirect_uri),
                ("response_type", "code"),
                ("scope", &scopes.join(" ")),
                ("access_type", "offline"),
                ("prompt", "consent"),
                ("include_granted_scopes", "true"),
                ("state", state),
            ],
        )?;
        Ok(url.into())
    }

    async fn token(&self, form: &[(&str, &str)]) -> Result<TokenAnswer, GoogleFault> {
        let inner = self.inner()?;
        let response = inner
            .client
            .post(&inner.token_url)
            .form(form)
            .send()
            .await
            .map_err(unavailable)?;
        let status = response.status().as_u16();
        let body = response.bytes().await.map_err(unavailable)?;
        open_token(status, &body)
    }

    /// Exchanges the `code` Google sent a merchant back with for a grant.
    /// `redirect_uri` is the one the merchant was sent with.
    pub async fn exchange(&self, code: &str, redirect_uri: &str) -> Result<Grant, GoogleFault> {
        let inner = self.inner()?;
        let answer = self
            .token(&[
                ("grant_type", "authorization_code"),
                ("code", code),
                ("redirect_uri", redirect_uri),
                ("client_id", &inner.client_id),
                ("client_secret", &inner.client_secret),
            ])
            .await?;
        Ok(Grant {
            refresh_token: answer.refresh_token.filter(|token| !token.is_empty()),
            access_token: answer.access_token,
            expires_in: answer.expires_in,
            accesses: accesses_of(answer.scope.split_whitespace()),
        })
    }

    async fn get<T: DeserializeOwned>(
        &self,
        url: reqwest::Url,
        access_token: &str,
    ) -> Result<T, GoogleFault> {
        let response = self
            .inner()?
            .client
            .get(url)
            .bearer_auth(access_token)
            .send()
            .await
            .map_err(unavailable)?;
        let status = response.status().as_u16();
        let body = response.bytes().await.map_err(unavailable)?;
        open_api(status, &body)
    }

    /// The e-mail address of the account an access was granted by.
    pub async fn account(&self, access_token: &str) -> Result<String, GoogleFault> {
        let url = reqwest::Url::parse(&self.inner()?.userinfo_url)
            .map_err(|err| GoogleFault::Refused(err.to_string()))?;
        let info: UserInfo = self.get(url, access_token).await?;
        Ok(info.email)
    }

    /// Seals a grant's refresh token for the shop it was given to.
    pub fn seal(&self, tenant_id: &str, refresh_token: &str) -> anyhow::Result<Vec<u8>> {
        self.inner()?.sealer.seal_for(tenant_id, refresh_token)
    }

    /// Keeps the access a grant came with, so the page the merchant lands
    /// on reads through it without asking for another.
    pub fn keep(&self, tenant_id: &str, refresh_token_sealed: &[u8], grant: &Grant) {
        let Ok(inner) = self.inner() else {
            return;
        };
        let held = Held {
            refresh_token_sealed: refresh_token_sealed.to_vec(),
            access_token: grant.access_token.clone(),
            until: Instant::now() + Duration::from_secs(grant.expires_in),
        };
        if let Ok(mut all) = inner.held.lock() {
            all.insert(tenant_id.to_owned(), held);
        }
    }

    /// An access to read Google with under the shop's grant: the one this
    /// node holds while it is good for a few minutes more, else another,
    /// asked for with the grant. A grant Google no longer honours is
    /// recorded as lapsed on the connection in `store`, and answers
    /// [`GoogleFault::ConnectAgain`]; so does one known as lapsed, unasked.
    pub async fn access<E: evento::Executor + Clone>(
        &self,
        store: &E,
        connection: &ConnectionState,
    ) -> Result<String, GoogleFault> {
        let inner = self.inner()?;
        if !connection.connected || connection.lapsed {
            return Err(GoogleFault::ConnectAgain);
        }
        let tenant_id = connection.id.as_str();
        if let Ok(all) = inner.held.lock()
            && let Some(held) = all.get(tenant_id)
            && held.refresh_token_sealed == connection.refresh_token_sealed
            && Instant::now() + RENEW_BEFORE < held.until
        {
            return Ok(held.access_token.clone());
        }
        let refresh_token = inner
            .sealer
            .open_for(tenant_id, &connection.refresh_token_sealed)
            .map_err(|err| {
                // Sealed under another key (the operator's secret changed):
                // only connecting again gives one this node can open.
                tracing::warn!(tenant = %tenant_id, %err, "a shop's Google grant does not open");
                GoogleFault::ConnectAgain
            });
        let renewed = match refresh_token {
            Ok(refresh_token) => {
                self.token(&[
                    ("grant_type", "refresh_token"),
                    ("refresh_token", &refresh_token),
                    ("client_id", &inner.client_id),
                    ("client_secret", &inner.client_secret),
                ])
                .await
            }
            Err(fault) => Err(fault),
        };
        match renewed {
            Ok(answer) => {
                let held = Held {
                    refresh_token_sealed: connection.refresh_token_sealed.clone(),
                    access_token: answer.access_token.clone(),
                    until: Instant::now() + Duration::from_secs(answer.expires_in),
                };
                if let Ok(mut all) = inner.held.lock() {
                    all.insert(tenant_id.to_owned(), held);
                }
                Ok(answer.access_token)
            }
            Err(GoogleFault::ConnectAgain) => {
                self.lapsed(store, connection).await;
                Err(GoogleFault::ConnectAgain)
            }
            Err(fault) => Err(fault),
        }
    }

    /// Drops the access this node holds for a shop.
    fn forget(&self, tenant_id: &str) {
        if let Ok(inner) = self.inner()
            && let Ok(mut all) = inner.held.lock()
        {
            all.remove(tenant_id);
        }
    }

    /// Makes `call` under the shop's access ([`access`](Self::access)). An
    /// access Google turns down (the account withdrew its yes within the
    /// hour) is dropped and another asked for, once: the grant refused then
    /// is recorded as lapsed, and one renewed makes the call again.
    pub async fn read<E, T, F, Fut>(
        &self,
        store: &E,
        connection: &ConnectionState,
        call: F,
    ) -> Result<T, GoogleFault>
    where
        E: evento::Executor + Clone,
        F: Fn(String) -> Fut,
        Fut: Future<Output = Result<T, GoogleFault>>,
    {
        let access = self.access(store, connection).await?;
        match call(access).await {
            Err(GoogleFault::ConnectAgain) => {
                self.forget(&connection.id);
                let access = self.access(store, connection).await?;
                call(access).await.map_err(|fault| match fault {
                    GoogleFault::ConnectAgain => {
                        GoogleFault::Unavailable("an access just renewed was turned down".into())
                    }
                    fault => fault,
                })
            }
            answer => answer,
        }
    }

    /// Records that Google no longer honours the grant `connection` holds,
    /// and drops the access held for it. A store that does not take the
    /// write leaves it to the next one who meets the refusal.
    async fn lapsed<E: evento::Executor + Clone>(&self, store: &E, connection: &ConnectionState) {
        self.forget(&connection.id);
        let noted = timada_google::Command(store.clone())
            .note_lapsed(
                GOOGLE_ACTOR,
                &connection.id,
                &connection.refresh_token_sealed,
            )
            .await;
        match noted {
            Ok(true) => {
                tracing::warn!(tenant = %connection.id, "Google no longer honours a shop's grant: its merchant has to connect again");
            }
            Ok(false) => {}
            Err(err) => {
                tracing::warn!(tenant = %connection.id, %err, "a lapsed Google grant could not be recorded");
            }
        }
    }
}

#[cfg(test)]
pub(crate) mod tests;
