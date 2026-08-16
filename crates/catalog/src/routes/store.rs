//! Storefront pages. This router is merged at the site root, so its paths are
//! the public URLs.

use askama::Template;
use axum::Router;
use axum::extract::{Path, State};
use axum::response::IntoResponse;
use axum::routing::get;
use sqlx::SqlitePool;
use timada_core::{AppError, AppResult, Currency, Money};
use timada_web::HtmlTemplate;

use crate::state::CatalogState;

/// The grid is unpaginated this pass; the cap keeps the page bounded.
const GRID_LIMIT: i64 = 100;

pub fn store_router(state: CatalogState) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/products/{id}", get(detail))
        .with_state(state)
}

/// Renders `amount_cents` with its currency, falling back to raw minor units
/// if the stored code is one this build doesn't know — an unrecognised code
/// shouldn't blank the page.
fn format_price(amount_cents: i64, currency: &str) -> String {
    match Currency::from_code(currency) {
        Ok(currency) => Money::new(amount_cents, currency).to_string(),
        Err(_) => format!("{amount_cents} {currency}"),
    }
}

/// One card in the storefront grid.
#[derive(sqlx::FromRow)]
struct ProductCard {
    id: String,
    title: String,
    price_cents: i64,
    currency: String,
    image_url: String,
}

impl ProductCard {
    fn price(&self) -> String {
        format_price(self.price_cents, &self.currency)
    }
}

#[derive(Template)]
#[template(path = "store/index.html")]
struct IndexTemplate {
    products: Vec<ProductCard>,
}

async fn index(State(state): State<CatalogState>) -> AppResult<impl IntoResponse> {
    let products = published_products(&state.ctx.read_pool).await?;

    Ok(HtmlTemplate(IndexTemplate { products }))
}

/// Only published products are in this table at all, so no status filter.
async fn published_products(read_pool: &SqlitePool) -> Result<Vec<ProductCard>, sqlx::Error> {
    sqlx::query_as(
        "SELECT id, title, price_cents, currency, image_url
           FROM store_product_list
          ORDER BY created_at DESC, id DESC
          LIMIT ?",
    )
    .bind(GRID_LIMIT)
    .fetch_all(read_pool)
    .await
}

/// The product page.
#[derive(sqlx::FromRow)]
struct ProductDetail {
    id: String,
    title: String,
    description: String,
    price_cents: i64,
    currency: String,
    image_url: String,
}

impl ProductDetail {
    fn price(&self) -> String {
        format_price(self.price_cents, &self.currency)
    }
}

#[derive(Template)]
#[template(path = "store/detail.html")]
struct DetailTemplate {
    product: ProductDetail,
}

async fn detail(
    State(state): State<CatalogState>,
    Path(id): Path<String>,
) -> AppResult<impl IntoResponse> {
    // Unpublished and archived products 404 rather than 403: the storefront
    // should not confirm that an id it won't sell exists.
    let product = published_product(&state.ctx.read_pool, &id)
        .await?
        .ok_or(AppError::NotFound)?;

    Ok(HtmlTemplate(DetailTemplate { product }))
}

async fn published_product(
    read_pool: &SqlitePool,
    id: &str,
) -> Result<Option<ProductDetail>, sqlx::Error> {
    sqlx::query_as(
        "SELECT id, title, description, price_cents, currency, image_url
           FROM store_product_detail
          WHERE id = ? AND published = 1",
    )
    .bind(id)
    .fetch_optional(read_pool)
    .await
}
