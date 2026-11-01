//! Queries over `pages`, for the pages list of a tenant.

use timada_core::db::{Db, FromRow};

/// One row of `pages`.
#[derive(Debug, Clone, FromRow, PartialEq, Eq)]
pub struct PageRow {
    pub tenant_id: String,
    pub id: String,
    pub slug: String,
    pub title: String,
    pub published: bool,
    /// Unix seconds.
    pub created_at: i64,
    /// Unix seconds.
    pub updated_at: i64,
}

/// The tenant's pages, by title.
pub async fn pages_of_tenant(db: &Db, tenant_id: &str) -> anyhow::Result<Vec<PageRow>> {
    db.fetch_all(
        "SELECT tenant_id, id, slug, title, published, created_at, updated_at FROM pages
         WHERE tenant_id = ?1
         ORDER BY title COLLATE NOCASE, id",
        (tenant_id,),
    )
    .await
}

/// How many pages the tenant has.
pub async fn count_pages(db: &Db, tenant_id: &str) -> anyhow::Result<u64> {
    let count: i64 = db
        .fetch_scalar(
            "SELECT COUNT(*) FROM pages WHERE tenant_id = ?1",
            (tenant_id,),
        )
        .await?;
    Ok(count as u64)
}
