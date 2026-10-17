//! The settings a storefront reads from its environment, so an app's `main`
//! is a few lines:
//!
//! - `STORE_ADDR` — the store API (`127.0.0.1:4321`): one node, or a host
//!   name in front of several, looked up again on every reconnect
//! - `STORE_TOKEN` — the API key's token, from the tenant's API keys page
//! - `STORE_DB` — the local SQLite file
//! - `STORE_TLS_CA` — TLS to the store (optional), checked against
//!   `STORE_TLS_SERVER_NAME` (the host of `STORE_ADDR` by default)
//! - `STORE_MEDIA_URL` — where the product pictures are loaded from (optional)
//! - `STORE_API_URL` — the dashboard address, where reviews are handed in and
//!   shoppers signed in (optional)
//! - `STORE_COOKIE_KEY` — at least 32 bytes the shopper cookies are sealed
//!   with; every replica shares it. Unset, a random one is made at startup
//!   and the cookies are lost on restart
//! - `STORE_SECURE_COOKIES` — `1` behind HTTPS: the cookies are `Secure`
//! - `STORE_SELLER_COUNTRY` — the merchant's country (`FR`), for the VAT
//!   line of the checkout
//! - `HOST` / `PORT` — where to listen (`127.0.0.1` and the app's default)
//! - `TRUSTED_PROXIES` — the reverse proxies in front of the app, as
//!   comma-separated addresses or CIDR networks, whose `X-Forwarded-For`
//!   names the shopper (optional)

use std::net::IpAddr;
use std::path::{Path, PathBuf};

use anyhow::Context as _;
use sqlx::SqlitePool;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
use tokio::net::TcpListener;
use topcoat_cookie::Key;
use topcoat_router::IpNet;

use crate::config::{StoreConfig, StoreTls};
use crate::store::Store;
use crate::storefront::{CookieOptions, Storefront};

/// What an app answers when its environment says nothing.
#[derive(Clone, Debug)]
pub struct EnvDefaults {
    /// The local SQLite file (`storefront.db`).
    pub db: &'static str,
    pub port: u16,
}

/// The settings read from the environment.
#[derive(Clone)]
pub struct StorefrontEnv {
    pub db: String,
    pub addr: String,
    pub token: String,
    pub tls_ca: Option<PathBuf>,
    pub tls_server_name: Option<String>,
    pub media_url: Option<String>,
    pub api_url: Option<String>,
    pub cookie_key: Key,
    pub secure_cookies: bool,
    pub seller_country: Option<String>,
    pub trusted_proxies: Vec<IpNet>,
    pub host: String,
    pub port: u16,
}

impl std::fmt::Debug for StorefrontEnv {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StorefrontEnv")
            .field("db", &self.db)
            .field("addr", &self.addr)
            .field("token", &"******")
            .field("tls_ca", &self.tls_ca)
            .field("tls_server_name", &self.tls_server_name)
            .field("media_url", &self.media_url)
            .field("api_url", &self.api_url)
            .field("cookie_key", &"******")
            .field("secure_cookies", &self.secure_cookies)
            .field("seller_country", &self.seller_country)
            .field("trusted_proxies", &self.trusted_proxies)
            .field("host", &self.host)
            .field("port", &self.port)
            .finish()
    }
}

fn env_or(name: &str, default: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| default.to_owned())
}

/// `TRUSTED_PROXIES` as a list: comma-separated addresses or CIDR networks.
pub fn parse_trusted_proxies(value: &str) -> anyhow::Result<Vec<IpNet>> {
    value
        .split(',')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .map(|entry| {
            entry
                .parse::<IpNet>()
                .or_else(|_| entry.parse::<IpAddr>().map(IpNet::from))
                .map_err(|_| {
                    anyhow::anyhow!(
                        "TRUSTED_PROXIES: {entry:?} is not an IP address or a CIDR network"
                    )
                })
        })
        .collect()
}

impl StorefrontEnv {
    /// Reads the environment; fails on a setting that cannot be used
    /// (`STORE_TOKEN` missing, a short cookie key, a port that is not one).
    pub fn from_env(defaults: EnvDefaults) -> anyhow::Result<Self> {
        let cookie_key = match std::env::var("STORE_COOKIE_KEY") {
            Ok(secret) if secret.len() >= 32 => Key::derive_from(secret.as_bytes()),
            Ok(_) => anyhow::bail!("STORE_COOKIE_KEY must be at least 32 bytes"),
            Err(_) => {
                tracing::warn!(
                    "STORE_COOKIE_KEY is not set: shopper cookies use a random key and are lost on restart"
                );
                Key::generate()
            }
        };
        let port: u16 = env_or("PORT", &defaults.port.to_string())
            .parse()
            .context("PORT is not a port")?;
        Ok(Self {
            db: env_or("STORE_DB", defaults.db),
            addr: env_or("STORE_ADDR", "127.0.0.1:4321"),
            token: std::env::var("STORE_TOKEN").context("STORE_TOKEN is not set")?,
            tls_ca: std::env::var("STORE_TLS_CA").ok().map(PathBuf::from),
            tls_server_name: std::env::var("STORE_TLS_SERVER_NAME").ok(),
            media_url: std::env::var("STORE_MEDIA_URL").ok(),
            api_url: std::env::var("STORE_API_URL").ok(),
            cookie_key,
            secure_cookies: matches!(
                env_or("STORE_SECURE_COOKIES", "").as_str(),
                "1" | "true" | "yes"
            ),
            seller_country: std::env::var("STORE_SELLER_COUNTRY")
                .ok()
                .filter(|code| !code.trim().is_empty()),
            trusted_proxies: parse_trusted_proxies(&env_or("TRUSTED_PROXIES", ""))?,
            host: env_or("HOST", "127.0.0.1"),
            port,
        })
    }

    /// Opens the local database (WAL, three connections).
    pub async fn open_pool(&self) -> anyhow::Result<SqlitePool> {
        SqlitePoolOptions::new()
            .max_connections(3)
            .connect_with(
                SqliteConnectOptions::new()
                    .filename(&self.db)
                    .create_if_missing(true)
                    .journal_mode(SqliteJournalMode::Wal),
            )
            .await
            .context("cannot open the local database")
    }

    /// The host of `STORE_ADDR`, without the port.
    fn store_host(&self) -> anyhow::Result<String> {
        let Some((host, _)) = self.addr.rsplit_once(':') else {
            anyhow::bail!("STORE_ADDR is not host:port: {:?}", self.addr);
        };
        Ok(host.trim_matches(['[', ']']).to_owned())
    }

    /// What [`Store::connect`] takes.
    pub fn store_config(&self) -> anyhow::Result<StoreConfig> {
        let mut config = StoreConfig::new(self.addr.clone(), self.token.clone());
        if let Some(ca) = &self.tls_ca {
            let server_name = match &self.tls_server_name {
                Some(name) => name.clone(),
                None => self.store_host()?,
            };
            config = config.tls(StoreTls::from_ca_pem(Path::new(ca), &server_name)?);
        }
        if let Some(media) = &self.media_url {
            config = config.media_url(media.clone());
        }
        if let Some(api) = &self.api_url {
            config = config.api_url(api.clone());
        }
        Ok(config)
    }

    /// The storefront to hand the router, with the key, the proxies and the
    /// cookie options the environment gave; the app adds its wording,
    /// paths and money format.
    pub fn storefront(&self, store: Store) -> Storefront {
        let mut storefront = Storefront::new(store)
            .cookie_key(self.cookie_key.clone())
            .trusted_proxies(self.trusted_proxies.clone())
            .cookies(CookieOptions {
                secure: self.secure_cookies,
                ..CookieOptions::default()
            });
        if let Some(country) = &self.seller_country {
            storefront = storefront.seller_country(country.clone());
        }
        storefront
    }

    /// Listens where `HOST` and `PORT` say.
    pub async fn bind(&self) -> anyhow::Result<TcpListener> {
        TcpListener::bind((self.host.as_str(), self.port))
            .await
            .with_context(|| format!("cannot bind {}:{}", self.host, self.port))
    }

    /// `http://host:port/`, for the startup log.
    pub fn url(&self) -> String {
        format!("http://{}:{}/", self.host, self.port)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proxies_take_addresses_and_networks() {
        let list = parse_trusted_proxies("10.0.0.0/8, 192.168.1.1,,fd00::/8").unwrap();
        assert_eq!(list.len(), 3);
        assert!(parse_trusted_proxies("nope").is_err());
        assert!(parse_trusted_proxies("").unwrap().is_empty());
    }
}
