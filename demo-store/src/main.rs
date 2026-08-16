//! Example storefront built on the timada framework.
//!
//! Demonstrates the full wiring: SQLite pools, migrations, the evento
//! executor, the provider registry, and mounting the drop-in admin under
//! `/admin` next to a fully custom storefront.

use std::sync::Arc;

use askama::Template;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use timada_provider::ProviderRegistry;
use timada_provider_aliexpress::AliExpress;
use tracing_subscriber::EnvFilter;

const STORE_CSS: &str = include_str!("../assets/store.css");

#[derive(Debug, thiserror::Error)]
enum StoreError {
    #[error(transparent)]
    Internal(#[from] anyhow::Error),
}

impl IntoResponse for StoreError {
    fn into_response(self) -> Response {
        let StoreError::Internal(error) = self;
        tracing::error!(error = ?error, "storefront request failed");
        (StatusCode::INTERNAL_SERVER_ERROR, "Internal server error").into_response()
    }
}

fn html<T: Template>(template: &T) -> Result<Response, StoreError> {
    let body = template.render().map_err(anyhow::Error::from)?;
    Ok(([(header::CONTENT_TYPE, "text/html; charset=utf-8")], body).into_response())
}

#[derive(Template)]
#[template(path = "home.html")]
struct HomePage {}

async fn home() -> Result<Response, StoreError> {
    html(&HomePage {})
}

async fn store_css() -> impl IntoResponse {
    (
        [
            (header::CONTENT_TYPE, "text/css; charset=utf-8"),
            (header::CACHE_CONTROL, "public, max-age=3600"),
        ],
        STORE_CSS,
    )
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let database_url =
        std::env::var("DATABASE_URL").unwrap_or_else(|_| "sqlite:timada.db?mode=rwc".to_owned());

    // Migrations run on a short-lived CLI pool with a long busy timeout,
    // before the serve pools open.
    {
        let pool = timada::db::create_pool(&database_url, 1).await?;
        timada::db::migrate(&pool).await?;
        pool.close().await;
    }

    let cores = std::thread::available_parallelism().map_or(4, |n| n.get() as u32);
    let read_pool = timada::db::create_read_pool(&database_url, cores).await?;
    let write_pool = timada::db::create_write_pool(&database_url).await?;
    let executor: evento::sql::RwSqlite =
        (read_pool.clone().into(), write_pool.clone().into()).into();

    let providers = Arc::new(ProviderRegistry::default().register(Arc::new(AliExpress)));

    let subscriptions = timada::subscriptions::start(&executor, write_pool.clone()).await?;

    let admin = timada_admin::AdminContext::new(executor, read_pool, Arc::clone(&providers));

    let app = axum::Router::new()
        .route("/", get(home))
        .route("/assets/store.css", get(store_css))
        .nest("/admin", timada_admin::router(admin));

    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await?;
    tracing::info!("demo-store listening on http://localhost:3000");

    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            if let Err(error) = tokio::signal::ctrl_c().await {
                tracing::error!(error = ?error, "failed to listen for shutdown signal");
            }
        })
        .await?;

    subscriptions.shutdown().await?;
    Ok(())
}
