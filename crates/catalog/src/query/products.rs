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

/// The products of a tenant whose slug or name contains `q` (all of them
/// when `None`), most recently changed first.
pub async fn products_of_tenant(
    pool: &SqlitePool,
    tenant_id: &str,
    q: Option<&str>,
    offset: i64,
    limit: i64,
) -> sqlx::Result<Vec<ProductRow>> {
    sqlx::query_as(
        "SELECT tenant_id, id, slug, name, description, amount_cents, currency, status,
            created_by, created_at, updated_at
         FROM catalog_products
         WHERE tenant_id = ?1
           AND (?2 IS NULL OR slug LIKE ?2 ESCAPE '\\' OR name LIKE ?2 ESCAPE '\\')
         ORDER BY updated_at DESC, id DESC
         LIMIT ?3 OFFSET ?4",
    )
    .bind(tenant_id)
    .bind(q.map(contains))
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await
}

/// How many products [`products_of_tenant`] would list.
pub async fn count_products(
    pool: &SqlitePool,
    tenant_id: &str,
    q: Option<&str>,
) -> sqlx::Result<i64> {
    sqlx::query_scalar(
        "SELECT count(*) FROM catalog_products
         WHERE tenant_id = ?1
           AND (?2 IS NULL OR slug LIKE ?2 ESCAPE '\\' OR name LIKE ?2 ESCAPE '\\')",
    )
    .bind(tenant_id)
    .bind(q.map(contains))
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
