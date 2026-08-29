//! Storefront pages. This router is merged at the site root, so its paths are
//! the public URLs.
//!
//! Both pages are region-aware: the browser's region (cookie, first region as
//! fallback) picks the currency, the grid shows only products priced in it —
//! the Medusa behaviour — and the detail page says so instead of quoting a
//! currency the shopper did not choose. A store with no regions at all falls
//! back to each product's base price, so nothing here requires seeding.

use askama::Template;
use axum::Router;
use axum::extract::{Path, State};
use axum::response::IntoResponse;
use axum::routing::get;
use axum_extra::extract::cookie::CookieJar;
use sqlx::SqlitePool;
use timada_core::{AppError, AppResult, Currency, Money};
use timada_region::current_region;
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

async fn index(State(state): State<CatalogState>, jar: CookieJar) -> AppResult<impl IntoResponse> {
    let region = current_region(&state.ctx.read_pool, &jar).await?;
    let products = match &region {
        Some(region) => products_in_currency(&state.ctx.read_pool, &region.currency).await?,
        None => published_products(&state.ctx.read_pool).await?,
    };

    Ok(HtmlTemplate(IndexTemplate { products }))
}

/// Published products priced in the region's currency — a product with no
/// price there is hidden, not shown in a currency the shopper did not pick.
async fn products_in_currency(
    read_pool: &SqlitePool,
    currency: &str,
) -> Result<Vec<ProductCard>, sqlx::Error> {
    sqlx::query_as(
        "SELECT p.id, p.title, pr.amount_cents AS price_cents, pr.currency, p.image_url
           FROM store_product_list p
           JOIN store_product_prices pr ON pr.product_id = p.id AND pr.currency = ?
          ORDER BY p.created_at DESC, p.id DESC
          LIMIT ?",
    )
    .bind(currency)
    .bind(GRID_LIMIT)
    .fetch_all(read_pool)
    .await
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

#[derive(Template)]
#[template(path = "store/detail.html")]
struct DetailTemplate {
    product: ProductDetail,
    /// The formatted price in the shopper's currency; `None` means the
    /// product is not for sale in their region and the add form is hidden.
    price: Option<String>,
}

async fn detail(
    State(state): State<CatalogState>,
    jar: CookieJar,
    Path(id): Path<String>,
) -> AppResult<impl IntoResponse> {
    // Unpublished and archived products 404 rather than 403: the storefront
    // should not confirm that an id it won't sell exists.
    let product = published_product(&state.ctx.read_pool, &id)
        .await?
        .ok_or(AppError::NotFound)?;

    let price = match current_region(&state.ctx.read_pool, &jar).await? {
        Some(region) => price_in_currency(&state.ctx.read_pool, &id, &region.currency)
            .await?
            .map(|amount_cents| format_price(amount_cents, &region.currency)),
        None => Some(format_price(product.price_cents, &product.currency)),
    };

    Ok(HtmlTemplate(DetailTemplate { product, price }))
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

async fn price_in_currency(
    read_pool: &SqlitePool,
    product_id: &str,
    currency: &str,
) -> Result<Option<i64>, sqlx::Error> {
    let row: Option<(i64,)> = sqlx::query_as(
        "SELECT amount_cents FROM store_product_prices WHERE product_id = ? AND currency = ?",
    )
    .bind(product_id)
    .bind(currency)
    .fetch_optional(read_pool)
    .await?;

    Ok(row.map(|(amount_cents,)| amount_cents))
}
