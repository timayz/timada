//! Queries over `pages`, for the pages list of a tenant.

use sqlx::{FromRow, SqlitePool};

/// One row of `pages`.
#[derive(Debug, Clone, FromRow, PartialEq, Eq)]
pub struct PageRow {
    pub tenant_id: String,
    pub id: String,
    pub slug: String,
    pub title: String,
    pub published: bool,
    pub footer_position: Option<i64>,
    /// Unix seconds.
    pub created_at: i64,
    /// Unix seconds.
    pub updated_at: i64,
}

/// The tenant's pages: those in the footer first, in its order, then the
/// others by title.
pub async fn pages_of_tenant(pool: &SqlitePool, tenant_id: &str) -> anyhow::Result<Vec<PageRow>> {
    Ok(sqlx::query_as(
        "SELECT * FROM pages WHERE tenant_id = ?1
         ORDER BY footer_position IS NULL, footer_position, title COLLATE NOCASE, id",
    )
    .bind(tenant_id)
    .fetch_all(pool)
    .await?)
}

/// How many pages the tenant has.
pub async fn count_pages(pool: &SqlitePool, tenant_id: &str) -> anyhow::Result<u64> {
    let (count,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM pages WHERE tenant_id = ?1")
        .bind(tenant_id)
        .fetch_one(pool)
        .await?;
    Ok(count as u64)
}
