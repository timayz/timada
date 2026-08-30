//! Admin pages for the catalog, mounted by the umbrella crate at
//! `/admin/catalog`.
//!
//! Links and form actions are absolute `/admin/catalog/...` because the router
//! does not know its own mount point — the documented limitation of this pass.

use askama::Template;
use axum::Router;
use axum::extract::{Form, Path, Query, State};
use axum::response::{IntoResponse, Redirect};
use axum::routing::{get, post};
use sqlx::SqlitePool;
use timada_core::{AppError, AppResult, Currency, Money};
use timada_dropship::SupplierProduct;
use timada_web::HtmlTemplate;

use crate::commands::{archive_product, import_product, publish_product, set_product_price};
use crate::state::CatalogState;

/// Enough rows to see what is happening without paginating.
const RECENT_LIMIT: i64 = 100;

/// Where every write in this router lands the admin back.
const INDEX_PATH: &str = "/admin/catalog";

pub fn admin_router(state: CatalogState) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/products/{id}/publish", post(publish))
        .route("/products/{id}/archive", post(archive))
        .route("/products/{id}/price", post(set_price))
        .route("/import", get(import_page).post(import))
        .route("/import/search", get(import_search))
        .with_state(state)
}

/// One row of `admin_product_list`.
#[derive(sqlx::FromRow)]
struct ProductRow {
    id: String,
    title: String,
    price_cents: i64,
    currency: String,
    supplier_id: String,
    supplier_product_ref: String,
    /// `draft` | `published` | `archived`.
    status: String,
}

impl ProductRow {
    /// Falls back to raw minor units if the stored currency code is one this
    /// build doesn't know — an unrecognised code shouldn't blank the page.
    fn price(&self) -> String {
        match Currency::from_code(&self.currency) {
            Ok(currency) => Money::new(self.price_cents, currency).to_string(),
            Err(_) => format!("{} {}", self.price_cents, self.currency),
        }
    }

    fn status_class(&self) -> &'static str {
        match self.status.as_str() {
            "published" => "bg-emerald-100 text-emerald-800",
            "archived" => "bg-stone-200 text-stone-700",
            _ => "bg-amber-100 text-amber-900",
        }
    }
}

#[derive(Template)]
#[template(path = "admin/catalog/index.html")]
struct IndexTemplate {
    products: Vec<ProductRow>,
}

async fn index(State(state): State<CatalogState>) -> AppResult<impl IntoResponse> {
    let products = recent_products(&state.ctx.read_pool).await?;

    Ok(HtmlTemplate(IndexTemplate { products }))
}

/// Reads the eventually-consistent admin projection — a product that was just
/// published may take a beat to change status here.
async fn recent_products(read_pool: &SqlitePool) -> Result<Vec<ProductRow>, sqlx::Error> {
    sqlx::query_as(
        "SELECT id, title, price_cents, currency, supplier_id, supplier_product_ref, status
           FROM admin_product_list
          ORDER BY created_at DESC, id DESC
          LIMIT ?",
    )
    .bind(RECENT_LIMIT)
    .fetch_all(read_pool)
    .await
}

async fn publish(State(state): State<CatalogState>, Path(id): Path<String>) -> AppResult<Redirect> {
    publish_product(&state.ctx.executor, &id).await?;

    Ok(Redirect::to(INDEX_PATH))
}

async fn archive(State(state): State<CatalogState>, Path(id): Path<String>) -> AppResult<Redirect> {
    archive_product(&state.ctx.executor, &id).await?;

    Ok(Redirect::to(INDEX_PATH))
}

#[derive(serde::Deserialize)]
struct SetPriceForm {
    currency: String,
    /// Minor units — cents — matching how every price is stored.
    amount_cents: i64,
}

async fn set_price(
    State(state): State<CatalogState>,
    Path(id): Path<String>,
    Form(form): Form<SetPriceForm>,
) -> AppResult<Redirect> {
    let currency = Currency::from_code(form.currency.trim())
        .map_err(|source| AppError::BadRequest(source.to_string()))?;
    if form.amount_cents <= 0 {
        return Err(AppError::BadRequest(
            "the price must be a positive amount in cents".to_owned(),
        ));
    }

    set_product_price(
        &state.ctx.executor,
        &id,
        Money::new(form.amount_cents, currency),
    )
    .await
    .map_err(|source| AppError::BadRequest(source.to_string()))?;

    Ok(Redirect::to(INDEX_PATH))
}

/// Query params of the import page and of its results fragment.
///
/// `supplier_id` is optional so the page renders before anything is picked;
/// the fragment route rejects a missing one.
#[derive(serde::Deserialize)]
struct SearchQuery {
    supplier_id: Option<String>,
    #[serde(default)]
    q: String,
}

#[derive(Template)]
#[template(path = "admin/catalog/import.html")]
struct ImportTemplate {
    suppliers: Vec<&'static str>,
    supplier_id: String,
    query: String,
    searched: bool,
    products: Vec<SupplierProduct>,
}

/// The same block, rendered on its own for TwinSpark to swap into `#results`.
#[derive(Template)]
#[template(path = "admin/catalog/import.html", block = "results")]
struct ImportResultsTemplate {
    supplier_id: String,
    searched: bool,
    products: Vec<SupplierProduct>,
}

/// Full import page. It also answers the no-JS path: the search form is a
/// plain `GET` back here, so submitting it without TwinSpark re-renders the
/// page with its results already filled in.
async fn import_page(
    State(state): State<CatalogState>,
    Query(query): Query<SearchQuery>,
) -> AppResult<impl IntoResponse> {
    let supplier_id = query.supplier_id.unwrap_or_default();
    let products = if supplier_id.is_empty() {
        Vec::new()
    } else {
        search(&state, &supplier_id, &query.q).await?
    };

    Ok(HtmlTemplate(ImportTemplate {
        suppliers: state.registry.ids(),
        searched: !supplier_id.is_empty(),
        supplier_id,
        query: query.q,
        products,
    }))
}

/// Results fragment only — this is what `ts-req` on the search form hits.
async fn import_search(
    State(state): State<CatalogState>,
    Query(query): Query<SearchQuery>,
) -> AppResult<impl IntoResponse> {
    let supplier_id = query
        .supplier_id
        .filter(|supplier_id| !supplier_id.is_empty())
        .ok_or_else(|| AppError::BadRequest("pick a supplier to search".to_owned()))?;

    let products = search(&state, &supplier_id, &query.q).await?;

    Ok(HtmlTemplate(ImportResultsTemplate {
        supplier_id,
        searched: true,
        products,
    }))
}

/// An id that isn't registered is the admin picking a stale option, not a bug.
async fn search(
    state: &CatalogState,
    supplier_id: &str,
    query: &str,
) -> AppResult<Vec<SupplierProduct>> {
    let supplier = state
        .registry
        .get(supplier_id)
        .map_err(|source| AppError::BadRequest(source.to_string()))?;

    Ok(supplier.search_products(query).await?)
}

/// The hidden fields carried by each result card's import form.
///
/// The supplier's data round-trips through the browser rather than being
/// re-fetched, so what the admin saw is exactly what gets imported.
#[derive(serde::Deserialize)]
struct ImportForm {
    supplier_id: String,
    supplier_product_ref: String,
    title: String,
    description: String,
    price_cents: i64,
    currency: String,
    image_url: String,
}

async fn import(
    State(state): State<CatalogState>,
    Form(form): Form<ImportForm>,
) -> AppResult<Redirect> {
    let currency = Currency::from_code(&form.currency)
        .map_err(|source| AppError::BadRequest(source.to_string()))?;

    import_product(
        &state.ctx.executor,
        &form.supplier_id,
        SupplierProduct {
            supplier_product_ref: form.supplier_product_ref,
            title: form.title,
            description: form.description,
            price: Money::new(form.price_cents, currency),
            image_url: form.image_url,
        },
    )
    .await?;

    Ok(Redirect::to(INDEX_PATH))
}

#[cfg(test)]
mod tests {
    use super::*;
    use timada_core::{Currency, Money};

    fn lamp() -> SupplierProduct {
        SupplierProduct {
            supplier_product_ref: "MP-1001".to_owned(),
            title: "Aurora Desk Lamp".to_owned(),
            description: "Warm dimmable LED lamp.".to_owned(),
            price: Money::new(3499, Currency::Eur),
            image_url: "https://placehold.co/400x400".to_owned(),
        }
    }

    /// TwinSpark replaces the target with the reply's single root element, so
    /// the fragment must be exactly the `#results` div — no layout, no form.
    #[test]
    fn the_results_fragment_is_a_single_swappable_root() {
        let html = ImportResultsTemplate {
            supplier_id: "mock".to_owned(),
            searched: true,
            products: vec![lamp()],
        }
        .render()
        .unwrap();

        assert!(!html.contains("<!doctype html>"));
        assert!(!html.contains("ts-req"));
        assert_eq!(html.trim().matches("<div id=\"results\"").count(), 1);
        assert!(html.trim().starts_with("<div id=\"results\""));

        // The card round-trips the supplier's data back to `POST /import`.
        assert!(html.contains(r#"name="supplier_id" value="mock""#));
        assert!(html.contains(r#"name="price_cents" value="3499""#));
        assert!(html.contains(r#"name="currency" value="EUR""#));
        assert!(html.contains("34.99 EUR"));
    }

    #[test]
    fn the_import_page_renders_the_search_form_and_an_idle_results_block() {
        let html = ImportTemplate {
            suppliers: vec!["mock"],
            supplier_id: String::new(),
            query: String::new(),
            searched: false,
            products: Vec::new(),
        }
        .render()
        .unwrap();

        assert!(html.contains(r#"ts-req="/admin/catalog/import/search""#));
        assert!(html.contains(r##"ts-target="#results""##));
        assert!(html.contains(r#"<div id="results""#));
        assert!(html.contains("Pick a supplier and search"));
    }

    fn row(status: &str, currency: &str) -> ProductRow {
        ProductRow {
            id: "p1".to_owned(),
            title: "Aurora Desk Lamp".to_owned(),
            price_cents: 3499,
            currency: currency.to_owned(),
            supplier_id: "mock".to_owned(),
            supplier_product_ref: "MP-1001".to_owned(),
            status: status.to_owned(),
        }
    }

    fn admin_index(status: &str) -> String {
        IndexTemplate {
            products: vec![row(status, "EUR")],
        }
        .render()
        .unwrap()
    }

    #[test]
    fn the_admin_table_offers_publish_only_for_drafts() {
        let draft = admin_index("draft");
        assert!(draft.contains("/admin/catalog/products/p1/publish"));
        assert!(draft.contains("/admin/catalog/products/p1/archive"));
        assert!(draft.contains("34.99 EUR"));

        let published = admin_index("published");
        assert!(!published.contains("/admin/catalog/products/p1/publish"));
        assert!(published.contains("/admin/catalog/products/p1/archive"));

        let archived = admin_index("archived");
        assert!(!archived.contains("/admin/catalog/products/p1/publish"));
        assert!(!archived.contains("/admin/catalog/products/p1/archive"));
    }

    /// An unknown currency code must degrade to raw minor units, not blank the
    /// page.
    #[test]
    fn an_unknown_currency_falls_back_to_minor_units() {
        assert_eq!(row("draft", "JPY").price(), "3499 JPY");
        assert_eq!(
            row("draft", "EUR").status_class(),
            "bg-amber-100 text-amber-900"
        );
    }
}
