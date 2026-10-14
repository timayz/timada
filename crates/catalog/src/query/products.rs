//! Queries over `catalog_products`, for the products pages of a tenant.

use sqlx::{FromRow, SqlitePool};

use crate::command::{Price, ProductStatus};

/// One row of `catalog_products`.
#[derive(Debug, Clone, FromRow, PartialEq, Eq)]
pub struct ProductRow {
    pub tenant_id: String,
    pub id: String,
    pub slug: String,
    pub name: String,
    pub description: String,
    pub amount_cents: Option<i64>,
    pub currency: Option<String>,
    pub status: String,
    pub created_by: String,
    /// Unix seconds.
    pub created_at: i64,
    /// Unix seconds.
    pub updated_at: i64,
}

impl ProductRow {
    pub fn price(&self) -> Option<Price> {
        Some(Price {
            amount_cents: self.amount_cents? as u64,
            currency: self.currency.clone()?,
        })
    }

    pub fn status(&self) -> ProductStatus {
        ProductStatus::parse(&self.status).unwrap_or_default()
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

/// Which products a page lists: a search over slug and name, and one
/// category they must belong to. Both are optional; the default matches
/// every product of the tenant.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProductFilter<'a> {
    /// Matched anywhere in the slug or the name.
    pub q: Option<&'a str>,
    /// The id of a category the product must be in.
    pub category: Option<&'a str>,
}

impl<'a> ProductFilter<'a> {
    pub fn search(mut self, q: Option<&'a str>) -> Self {
        self.q = q.map(str::trim).filter(|q| !q.is_empty());
        self
    }

    pub fn category(mut self, category: Option<&'a str>) -> Self {
        self.category = category.map(str::trim).filter(|c| !c.is_empty());
        self
    }
}

/// The products of a tenant the filter matches, most recently changed first.
pub async fn products_of_tenant(
    pool: &SqlitePool,
    tenant_id: &str,
    filter: &ProductFilter<'_>,
    offset: i64,
    limit: i64,
) -> sqlx::Result<Vec<ProductRow>> {
    sqlx::query_as(
        "SELECT tenant_id, id, slug, name, description, amount_cents, currency, status,
            created_by, created_at, updated_at
         FROM catalog_products
         WHERE tenant_id = ?1
           AND (?2 IS NULL OR slug LIKE ?2 ESCAPE '\\' OR name LIKE ?2 ESCAPE '\\')
           AND (?3 IS NULL OR id IN (SELECT product_id FROM catalog_product_categories
                WHERE tenant_id = ?1 AND category_id = ?3))
         ORDER BY updated_at DESC, id DESC
         LIMIT ?4 OFFSET ?5",
    )
    .bind(tenant_id)
    .bind(filter.q.map(contains))
    .bind(filter.category)
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await
}

/// How many products [`products_of_tenant`] would list.
pub async fn count_products(
    pool: &SqlitePool,
    tenant_id: &str,
    filter: &ProductFilter<'_>,
) -> sqlx::Result<i64> {
    sqlx::query_scalar(
        "SELECT count(*) FROM catalog_products
         WHERE tenant_id = ?1
           AND (?2 IS NULL OR slug LIKE ?2 ESCAPE '\\' OR name LIKE ?2 ESCAPE '\\')
           AND (?3 IS NULL OR id IN (SELECT product_id FROM catalog_product_categories
                WHERE tenant_id = ?1 AND category_id = ?3))",
    )
    .bind(tenant_id)
    .bind(filter.q.map(contains))
    .bind(filter.category)
    .fetch_one(pool)
    .await
}

/// One product of a tenant, by id.
pub async fn product_by_id(
    pool: &SqlitePool,
    tenant_id: &str,
    id: &str,
) -> sqlx::Result<Option<ProductRow>> {
    sqlx::query_as(
        "SELECT tenant_id, id, slug, name, description, amount_cents, currency, status,
            created_by, created_at, updated_at
         FROM catalog_products WHERE tenant_id = ?1 AND id = ?2",
    )
    .bind(tenant_id)
    .bind(id)
    .fetch_optional(pool)
    .await
}
