//! What a storefront page reads: the published products of the local copy.

use sqlx::{FromRow, SqlitePool};
use timada_catalog::format_money;

/// A product as the storefront shows it.
#[derive(Debug, Clone, FromRow, PartialEq, Eq, Hash)]
pub struct Product {
    pub id: String,
    pub slug: String,
    pub name: String,
    pub description: String,
    pub amount_cents: Option<i64>,
    pub currency: Option<String>,
    /// Unix seconds of the last change.
    pub updated_at: i64,
}

impl Product {
    /// The price as a person reads it (`12.50 EUR`), once it has one.
    pub fn price(&self) -> Option<String> {
        Some(format_money(
            self.amount_cents? as u64,
            self.currency.as_deref()?,
        ))
    }
}

/// How many products a page shows unless asked otherwise.
pub const DEFAULT_PER_PAGE: u32 = 24;
pub const MAX_PER_PAGE: u32 = 100;

/// A page of the catalog: 1-based `page`, `per_page` capped at
/// [`MAX_PER_PAGE`], and an optional search over name and slug.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ProductQuery {
    pub page: u32,
    pub per_page: u32,
    pub q: Option<String>,
}

impl Default for ProductQuery {
    fn default() -> Self {
        Self {
            page: 1,
            per_page: DEFAULT_PER_PAGE,
            q: None,
        }
    }
}

impl ProductQuery {
    pub fn page(mut self, page: u32) -> Self {
        self.page = page.max(1);
        self
    }

    pub fn per_page(mut self, per_page: u32) -> Self {
        self.per_page = per_page.clamp(1, MAX_PER_PAGE);
        self
    }

    pub fn search(mut self, q: impl Into<String>) -> Self {
        let q = q.into();
        self.q = (!q.trim().is_empty()).then(|| q.trim().to_owned());
        self
    }
}

/// The products of one page and what is around it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProductPage {
    pub items: Vec<Product>,
    pub page: u32,
    pub per_page: u32,
    /// How many published products match, over every page.
    pub total: i64,
}

impl ProductPage {
    pub fn pages(&self) -> u32 {
        let per_page = i64::from(self.per_page.max(1));
        ((self.total + per_page - 1) / per_page).max(1) as u32
    }

    pub fn has_next(&self) -> bool {
        self.page < self.pages()
    }

    pub fn has_prev(&self) -> bool {
        self.page > 1
    }
}

/// A `LIKE` pattern matching `q` anywhere, with its wildcards escaped.
fn contains(q: &str) -> String {
    let escaped = q
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_");
    format!("%{escaped}%")
}

/// The published products, most recently changed first.
pub async fn published_products(
    pool: &SqlitePool,
    query: &ProductQuery,
) -> sqlx::Result<ProductPage> {
    let per_page = i64::from(query.per_page.clamp(1, MAX_PER_PAGE));
    let page = query.page.max(1);
    let pattern = query.q.as_deref().map(contains);
    let total: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM storefront_products
         WHERE status = 'published'
           AND (?1 IS NULL OR slug LIKE ?1 ESCAPE '\\' OR name LIKE ?1 ESCAPE '\\')",
    )
    .bind(&pattern)
    .fetch_one(pool)
    .await?;
    let items = sqlx::query_as(
        "SELECT id, slug, name, description, amount_cents, currency, updated_at
         FROM storefront_products
         WHERE status = 'published'
           AND (?1 IS NULL OR slug LIKE ?1 ESCAPE '\\' OR name LIKE ?1 ESCAPE '\\')
         ORDER BY updated_at DESC, id DESC
         LIMIT ?2 OFFSET ?3",
    )
    .bind(&pattern)
    .bind(per_page)
    .bind(i64::from(page - 1) * per_page)
    .fetch_all(pool)
    .await?;
    Ok(ProductPage {
        items,
        page,
        per_page: per_page as u32,
        total,
    })
}

/// One published product, by slug.
pub async fn published_product_by_slug(
    pool: &SqlitePool,
    slug: &str,
) -> sqlx::Result<Option<Product>> {
    sqlx::query_as(
        "SELECT id, slug, name, description, amount_cents, currency, updated_at
         FROM storefront_products WHERE slug = ?1 AND status = 'published'",
    )
    .bind(slug)
    .fetch_optional(pool)
    .await
}
