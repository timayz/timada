//! A merchant's storefront, as small as it gets: connects to a timada store
//! with an API key and serves the published catalog in plain HTML. No asset
//! bundle, no stylesheet build: `cargo run -p storefront-example` is enough.
//!
//! Configuration comes from the environment:
//!
//! - `STORE_ADDR` — the store API of the timada node (`127.0.0.1:4321`)
//! - `STORE_TOKEN` — the API key's token, from the tenant's API keys page
//! - `STORE_DB` — the local SQLite file (`storefront.db`)
//! - `STORE_TLS_CA` + `STORE_TLS_SERVER_NAME` — TLS to the store (optional)
//! - `STORE_MEDIA_URL` — where the product pictures are loaded from, as the
//!   tenant's API keys page shows it (optional)
//! - `HOST` / `PORT` — where to listen (`127.0.0.1` / `3456`)
//!
//! `storefront migrate` prepares the local database and exits, without
//! reaching the store.

mod app;

use std::net::SocketAddr;
use std::path::Path;

use anyhow::Context as _;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
use timada_storefront::{Store, StoreConfig, StoreTls};

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

    let addr: SocketAddr = env_or("STORE_ADDR", "127.0.0.1:4321")
        .parse()
        .context("STORE_ADDR is not an address")?;
    let token = std::env::var("STORE_TOKEN").context("STORE_TOKEN is not set")?;
    let mut config = StoreConfig::new(addr, token);
    if let Ok(ca) = std::env::var("STORE_TLS_CA") {
        let server_name = env_or("STORE_TLS_SERVER_NAME", "localhost");
        config = config.tls(StoreTls::from_ca_pem(Path::new(&ca), &server_name)?);
    }
    if let Ok(media) = std::env::var("STORE_MEDIA_URL") {
        config = config.media_url(media);
    }

    let store = Store::connect(config, pool).await?;

    let host = env_or("HOST", "127.0.0.1");
    let port: u16 = env_or("PORT", "3456")
        .parse()
        .context("PORT is not a port")?;
    let listener = tokio::net::TcpListener::bind((host.as_str(), port))
        .await
        .with_context(|| format!("cannot bind {host}:{port}"))?;
    tracing::info!("storefront at http://{host}:{port}/");
    topcoat::serve(listener, app::router(store.clone())).await?;

    store.shutdown().await;
    Ok(())
}
