//! Chatounette, the clone: a French cat shop in chatounette.com's layout,
//! built on the `timada-storefront` library. It connects to a timada store
//! with an API key and serves the published catalogue in plain HTML. No asset
//! bundle, no stylesheet build: `cargo run -p chatounette-example` is enough.
//!
//! It is meant to run against a tenant filled by `make seed.chatounette`,
//! which imports the shop's catalogue and prints the `STORE_*` values.
//!
//! Configuration comes from the environment, as `timada_storefront::env`
//! documents it (`STORE_ADDR`, `STORE_TOKEN`, `STORE_DB` (`chatounette.db`),
//! `STORE_TLS_CA`, `STORE_MEDIA_URL`, `STORE_API_URL`, `STORE_COOKIE_KEY`,
//! `HOST` / `PORT` (`127.0.0.1` / `3457`), `TRUSTED_PROXIES`). Selling (the
//! cart and checkout) needs a key holding `store:checkout`.
//!
//! `chatounette migrate` prepares the local database and exits, without
//! reaching the store.

mod app;
mod messages;

use timada_storefront::{EnvDefaults, MoneyFormat, Paths, Store, StorefrontEnv};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let env = StorefrontEnv::from_env(EnvDefaults {
        db: "chatounette.db",
        port: 3457,
    })?;
    let pool = env.open_pool().await?;
    if std::env::args().nth(1).as_deref() == Some("migrate") {
        timada_storefront::migrate(&pool).await?;
        tracing::info!("local database ready");
        return Ok(());
    }

    let store = Store::connect(env.store_config()?, pool).await?;
    let storefront = env
        .storefront(store.clone())
        .messages(messages::french())
        .money(MoneyFormat::french())
        .paths(Paths::default().catalog("/boutique"))
        .seller_country("FR")
        .accent_color("#f46063");

    let listener = env.bind().await?;
    tracing::info!("chatounette at {}", env.url());
    topcoat::serve(listener, app::router(storefront)).await?;

    store.shutdown().await;
    Ok(())
}
