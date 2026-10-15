//! A merchant's storefront, as small as it gets: connects to a timada store
//! with an API key and serves the published catalog in plain HTML. No asset
//! bundle, no stylesheet build: `cargo run -p storefront-example` is enough.
//!
//! Configuration comes from the environment:
//!
//! - `STORE_ADDR` — the store API (`127.0.0.1:4321`): one node, or a host name
//!   in front of several (a load balancer, a geo-routed name), looked up again
//!   on every reconnect
//! - `STORE_TOKEN` — the API key's token, from the tenant's API keys page
//! - `STORE_DB` — the local SQLite file (`storefront.db`)
//! - `STORE_TLS_CA` — TLS to the store (optional), checked against
//!   `STORE_TLS_SERVER_NAME` (the host of `STORE_ADDR` by default)
//! - `STORE_MEDIA_URL` — where the product pictures are loaded from, as the
//!   tenant's API keys page shows it (optional)
//! - `STORE_API_URL` — the dashboard address the tenant's API keys page
//!   shows, where shoppers' reviews are handed in (optional; without it the
//!   order page offers no review form)
//! - `STORE_COOKIE_KEY` — at least 32 bytes the shopper cookies (cart, past
//!   orders, last address) are encrypted with; every replica of the app shares
//!   it. Unset, a random one is made at startup, so the cookies of a browser
//!   are lost on restart
//! - `HOST` / `PORT` — where to listen (`127.0.0.1` / `3456`)
//!
//! Selling (the cart and checkout) needs a key holding `store:checkout`; with
//! a read-only key the catalog shows but adding to the cart fails.
//!
//! `storefront migrate` prepares the local database and exits, without
//! reaching the store.

mod app;
mod shopper;

use std::path::Path;

use anyhow::Context as _;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
use timada_storefront::{Store, StoreConfig, StoreTls};
use topcoat::cookie::Key;

fn env_or(name: &str, default: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| default.to_owned())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let pool = SqlitePoolOptions::new()
        .max_connections(3)
        .connect_with(
            SqliteConnectOptions::new()
                .filename(env_or("STORE_DB", "storefront.db"))
                .create_if_missing(true)
                .journal_mode(SqliteJournalMode::Wal),
        )
        .await
        .context("cannot open the local database")?;
    if std::env::args().nth(1).as_deref() == Some("migrate") {
        timada_storefront::migrate(&pool).await?;
        tracing::info!("local database ready");
        return Ok(());
    }

    let addr = env_or("STORE_ADDR", "127.0.0.1:4321");
    let Some((store_host, _)) = addr.rsplit_once(':') else {
        anyhow::bail!("STORE_ADDR is not host:port: {addr:?}");
    };
    let store_host = store_host.trim_matches(['[', ']']).to_owned();
    let token = std::env::var("STORE_TOKEN").context("STORE_TOKEN is not set")?;
    let mut config = StoreConfig::new(addr, token);
    if let Ok(ca) = std::env::var("STORE_TLS_CA") {
        let server_name = env_or("STORE_TLS_SERVER_NAME", &store_host);
        config = config.tls(StoreTls::from_ca_pem(Path::new(&ca), &server_name)?);
    }
    if let Ok(media) = std::env::var("STORE_MEDIA_URL") {
        config = config.media_url(media);
    }
    if let Ok(api) = std::env::var("STORE_API_URL") {
        config = config.api_url(api);
    }

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

    let store = Store::connect(config, pool).await?;

    let host = env_or("HOST", "127.0.0.1");
    let port: u16 = env_or("PORT", "3456")
        .parse()
        .context("PORT is not a port")?;
    let listener = tokio::net::TcpListener::bind((host.as_str(), port))
        .await
        .with_context(|| format!("cannot bind {host}:{port}"))?;
    tracing::info!("storefront at http://{host}:{port}/");
    topcoat::serve(listener, app::router(store.clone(), cookie_key)).await?;

    store.shutdown().await;
    Ok(())
}
