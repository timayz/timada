//! Demo storefront: a topcoat app over the timada contexts (catalogue, cart,
//! checkout, customer account) that mounts the admin under `/admin`.
//!
//! ```text
//! cargo run -p demo -- --seed          # sample catalog, customer, order, shopper and admin accounts
//! topcoat dev -p demo                  # bundle assets, watch, serve on :3000
//! ```

mod app;
mod auth;
mod cart_session;
mod db;
mod seed;
#[cfg(test)]
mod tests;

use std::env;

use topcoat::{
    asset::{AssetBundle, AssetCatalog, AssetConfig, RouterBuilderAssetExt},
    cookie::RouterBuilderCookieExt,
    router::{Router, RouterBuilderDiscoverExt},
    session::{RouterBuilderSessionExt, SessionConfig, cookie::CookieTokenStore},
};

use timada_admin::{AdminConfig, AdminServices, Stylesheet};

/// Shared by the storefront pages through app context.
#[derive(Clone)]
pub struct Store {
    pub executor: evento::Sqlite,
    pub db: sqlx::SqlitePool,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,sqlx=warn".into()),
        )
        .init();

    let args: Vec<String> = env::args().skip(1).collect();
    let (executor, pool) = db::open("data/demo.db").await?;
    let store = Store {
        executor: executor.clone(),
        db: pool.clone(),
    };

    match args.first().map(String::as_str) {
        Some("--seed") => {
            seed::run(&store).await?;
            db::run_subscriptions_once(&store).await?;
            tracing::info!("seeded; admin login is admin@timada.example / admin");
            tracing::info!(
                "shopper login is {} / {}",
                seed::SHOPPER_EMAIL,
                seed::SHOPPER_PASSWORD
            );
            return Ok(());
        }
        Some("--create-admin") => {
            let (email, password) = (args.get(1), args.get(2));
            let (Some(email), Some(password)) = (email, password) else {
                anyhow::bail!("usage: demo --create-admin <email> <password>");
            };
            let id = timada_admin::create_admin(&pool, email, password).await?;
            tracing::info!(%id, "admin created");
            return Ok(());
        }
        Some(other) => anyhow::bail!("unknown argument `{other}`"),
        None => {}
    }

    let _subscriptions = db::start_subscriptions(&store).await?;

    // `topcoat asset bundle --bin demo` (or `topcoat dev`) produces the bundle;
    // without it the storefront still serves, and the admin renders unstyled.
    let (assets, stylesheet) = match AssetBundle::load() {
        Ok(bundle) => (AssetConfig::from(bundle), Stylesheet::Bundled),
        Err(err) => {
            tracing::warn!(%err, "no asset bundle; run `topcoat asset bundle --bin demo`");
            (
                AssetConfig::hosted_at("/_topcoat/assets", AssetCatalog::default()),
                Stylesheet::Url("/missing-bundle.css".into()),
            )
        }
    };

    let router = router(store, assets, stylesheet);

    tracing::info!("storefront on http://127.0.0.1:3000, admin on /admin");
    topcoat::start(router).await?;
    Ok(())
}

/// The storefront pages (discovered) with shopper sessions, plus the admin.
fn router(store: Store, assets: AssetConfig, stylesheet: Stylesheet) -> Router {
    let sessions = SessionConfig::builder()
        .token_store(CookieTokenStore::new().name(auth::SESSION_COOKIE))
        .build();
    let services = AdminServices::new(store.executor.clone(), store.db.clone());
    let builder = Router::builder()
        .discover()
        .app_context(store)
        .cookies()
        .sessions(sessions)
        .assets(assets.clone());
    timada_admin::mount(
        builder,
        AdminConfig {
            mount: "admin".into(),
            stylesheet,
        },
        assets,
        services,
    )
    .build()
}
