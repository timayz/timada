//! Example storefront built on the timada framework.
//!
//! Demonstrates the full wiring: SQLite pools, migrations, the evento
//! executor, the provider registry, read-model subscriptions, and mounting
//! the drop-in admin under `/admin` next to a fully custom storefront.

use std::sync::Arc;

use askama::Template;
use axum::extract::{Path, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use timada::read_model::{catalog_detail, catalog_list};
use timada::self_inventory::SelfInventory;
use timada_provider::ProviderRegistry;
use timada_provider_aliexpress::AliExpress;
use tracing_subscriber::EnvFilter;

const STORE_CSS: &str = include_str!("../assets/store.css");

#[derive(Clone)]
struct StoreState {
    read_db: sqlx::SqlitePool,
}

#[derive(Debug, thiserror::Error)]
enum StoreError {
    #[error("not found")]
    NotFound,
    #[error(transparent)]
    Internal(#[from] anyhow::Error),
}

impl IntoResponse for StoreError {
    fn into_response(self) -> Response {
        match self {
            StoreError::NotFound => (StatusCode::NOT_FOUND, "Not found").into_response(),
            StoreError::Internal(error) => {
                tracing::error!(error = ?error, "storefront request failed");
                (StatusCode::INTERNAL_SERVER_ERROR, "Internal server error").into_response()
            }
        }
    }
}

fn html<T: Template>(template: &T) -> Result<Response, StoreError> {
    let body = template.render().map_err(anyhow::Error::from)?;
    Ok(([(header::CONTENT_TYPE, "text/html; charset=utf-8")], body).into_response())
}

fn format_amount(amount_minor: i64) -> String {
    format!("{}.{:02}", amount_minor / 100, (amount_minor % 100).abs())
}

struct ProductCard {
    id: String,
    title: String,
    thumbnail_url: String,
    price: String,
    currency: String,
}

#[derive(Template)]
#[template(path = "home.html")]
struct HomePage {
    products: Vec<ProductCard>,
}

async fn home(State(state): State<StoreState>) -> Result<Response, StoreError> {
    let page = catalog_list::page(&state.read_db, 24, None, None, true).await?;
    let products = page
        .edges
        .into_iter()
        .map(|edge| ProductCard {
            price: format_amount(edge.node.amount_minor),
            id: edge.node.id,
            title: edge.node.title,
            thumbnail_url: edge.node.thumbnail_url,
            currency: edge.node.currency,
        })
        .collect();

    html(&HomePage { products })
}

struct VariantView {
    title: String,
    price: String,
    currency: String,
}

#[derive(Template)]
#[template(path = "product.html")]
struct ProductPage {
    title: String,
    description: String,
    image_urls: Vec<String>,
    price: String,
    currency: String,
    in_stock: bool,
    variants: Vec<VariantView>,
}

async fn product(
    State(state): State<StoreState>,
    Path(id): Path<String>,
) -> Result<Response, StoreError> {
    let detail = catalog_detail::by_id(&state.read_db, &id)
        .await?
        .filter(|detail| detail.status == "published")
        .ok_or(StoreError::NotFound)?;

    let self_inventory = detail.provider_kind == timada::self_inventory::KIND;
    html(&ProductPage {
        title: detail.title,
        description: detail.description,
        image_urls: detail.image_urls,
        price: format_amount(detail.amount_minor),
        currency: detail.currency,
        // Imported (dropshipped) products are assumed available; their stock
        // lives at the provider.
        in_stock: !self_inventory || detail.stock_available > 0,
        variants: detail
            .variants
            .into_iter()
            .map(|variant| VariantView {
                title: variant.title,
                price: format_amount(variant.price_amount_minor),
                currency: variant.currency,
            })
            .collect(),
    })
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

    let providers = Arc::new(
        ProviderRegistry::default()
            .register(Arc::new(SelfInventory::new(read_pool.clone())))
            .register(Arc::new(AliExpress)),
    );

    let subscriptions = timada::subscriptions::start(&executor, write_pool.clone()).await?;

    let admin =
        timada_admin::AdminContext::new(executor, read_pool.clone(), Arc::clone(&providers));

    let app = axum::Router::new()
        .route("/", get(home))
        .route("/products/{id}", get(product))
        .route("/assets/store.css", get(store_css))
        .with_state(StoreState { read_db: read_pool })
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
