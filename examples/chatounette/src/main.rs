//! Chatounette, the clone: a French cat shop in chatounette.com's layout,
//! built on the `timada-storefront` library. It connects to a timada store
//! with an API key and serves the published catalogue in plain HTML. No asset
//! bundle, no stylesheet build: `cargo run -p chatounette-example` is enough.
//!
//! It is meant to run against a tenant filled by `make seed.chatounette`,
//! which imports the shop's catalogue and prints the `STORE_*` values below.
//!
//! Configuration comes from the environment:
//!
//! - `STORE_ADDR` — the store API (`127.0.0.1:4321`): one node, or a host name
//!   in front of several (a load balancer, a geo-routed name), looked up again
//!   on every reconnect
//! - `STORE_TOKEN` — the API key's token, from the tenant's API keys page
//! - `STORE_DB` — the local SQLite file (`chatounette.db`)
//! - `STORE_DB_READ_CONNECTIONS` — how many pages read the local file at once
//!   (8)
//! - `STORE_TLS_CA` — TLS to the store (optional), checked against
//!   `STORE_TLS_SERVER_NAME` (the host of `STORE_ADDR` by default)
//! - `STORE_MEDIA_URL` — where the product pictures are loaded from, as the
//!   tenant's API keys page shows it (optional)
//! - `STORE_API_URL` — the dashboard address the tenant's API keys page
//!   shows, where the shop sells (carts, payments, orders), hands in
//!   shoppers' reviews and signs shoppers in (without it the shop only
//!   shows its catalogue)
//! - `STORE_COOKIE_KEY` — at least 32 bytes the shopper cookies (cart, past
//!   orders, last address) are encrypted with; every replica of the app shares
//!   it. Unset, a random one is made at startup, so the cookies of a browser
//!   are lost on restart
//! - `HOST` / `PORT` — where to listen (`127.0.0.1` / `3457`)
//! - `TRUSTED_PROXIES` — the reverse proxies in front of the app, as
//!   comma-separated addresses or CIDR networks (`10.0.0.0/8,fd00::/8`), whose
//!   `X-Forwarded-For` names the shopper (optional; unset, the connection's
//!   peer is the shopper). The dashboard holds each shopper's address to a
//!   limit of sign-in codes, so behind a proxy this must be set, or every
//!   shopper shares the proxy's
//!
//! Selling (the cart and checkout) needs a key holding `store:checkout`; with
//! a read-only key the catalog shows but adding to the cart fails.
//!
//! `chatounette migrate` prepares the local database and exits, without
//! reaching the store.

mod app;
mod shopper;

use std::net::IpAddr;
use std::path::Path;

use anyhow::Context as _;
use timada_storefront::{MoneyFormat, Store, StoreConfig, StoreDb, StoreTls};
use topcoat::cookie::Key;
use topcoat::router::IpNet;

fn env_or(name: &str, default: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| default.to_owned())
}

/// `TRUSTED_PROXIES`: comma-separated addresses or CIDR networks.
fn trusted_proxies(value: &str) -> anyhow::Result<Vec<IpNet>> {
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

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let read_connections: u32 = env_or(
        "STORE_DB_READ_CONNECTIONS",
        &timada_storefront::DEFAULT_READ_CONNECTIONS.to_string(),
    )
    .parse()
    .context("STORE_DB_READ_CONNECTIONS is not a number")?;
    let db = StoreDb::open(env_or("STORE_DB", "chatounette.db"), read_connections)
        .await
        .context("cannot open the local database")?;
    if std::env::args().nth(1).as_deref() == Some("migrate") {
        timada_storefront::migrate(db.write()).await?;
        tracing::info!("local database ready");
        return Ok(());
    }

    let addr = env_or("STORE_ADDR", "127.0.0.1:4321");
    let Some((store_host, _)) = addr.rsplit_once(':') else {
        anyhow::bail!("STORE_ADDR is not host:port: {addr:?}");
    };
    let store_host = store_host.trim_matches(['[', ']']).to_owned();
    let token = std::env::var("STORE_TOKEN").context("STORE_TOKEN is not set")?;
    // Every total the library writes (cart, checkout, orders, shipping, offers)
    // looks like the prices on the shop's own pages.
    let mut config = StoreConfig::new(addr, token).money(
        MoneyFormat::new(|cents, currency| app::money(cents as i64, currency)).free("Offerte"),
    );
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

    let trusted_proxies = trusted_proxies(&env_or("TRUSTED_PROXIES", ""))?;

    let store = Store::connect(config, db).await?;

    let host = env_or("HOST", "127.0.0.1");
    let port: u16 = env_or("PORT", "3457")
        .parse()
        .context("PORT is not a port")?;
    let listener = tokio::net::TcpListener::bind((host.as_str(), port))
        .await
        .with_context(|| format!("cannot bind {host}:{port}"))?;
    tracing::info!("chatounette at http://{host}:{port}/");
    topcoat::serve(
        listener,
        app::router(store.clone(), cookie_key, trusted_proxies),
    )
    .await?;

    store.shutdown().await;
    Ok(())
}
