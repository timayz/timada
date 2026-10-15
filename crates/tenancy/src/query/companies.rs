//! Queries over `tenancy_companies`, for the companies pages.

use sqlx::{FromRow, SqlitePool};

/// One row of `tenancy_companies`.
#[derive(Debug, Clone, FromRow, PartialEq, Eq)]
pub struct CompanyRow {
    pub id: String,
    pub siren: String,
    pub legal_name: String,
    /// Unix seconds.
    pub created_at: i64,
}

/// A tenant selling as a company, with its series code.
#[derive(Debug, Clone, FromRow, PartialEq, Eq)]
pub struct CompanyTenantRow {
    pub id: String,
    pub slug: String,
    pub name: String,
    /// `""` for the plain series.
    pub series_code: String,
}

/// The companies a user owns, by name.
pub async fn companies_of_user(pool: &SqlitePool, user_id: &str) -> sqlx::Result<Vec<CompanyRow>> {
    sqlx::query_as(
        "SELECT c.* FROM tenancy_companies c
         JOIN tenancy_company_owners o ON o.company_id = c.id
         WHERE o.user_id = ?1 ORDER BY c.legal_name COLLATE NOCASE",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await
}

/// The tenants selling as a company, by name.
pub async fn tenants_of_company(
    pool: &SqlitePool,
    company_id: &str,
) -> sqlx::Result<Vec<CompanyTenantRow>> {
    sqlx::query_as(
        "SELECT id, slug, name, COALESCE(series_code, '') AS series_code FROM tenancy_tenants
         WHERE company_id = ?1 ORDER BY name COLLATE NOCASE",
    )
    .bind(company_id)
    .fetch_all(pool)
    .await
}

/// Every company, newest first, for the platform administrators.
pub async fn all_companies(
    pool: &SqlitePool,
    offset: i64,
    limit: i64,
) -> sqlx::Result<Vec<CompanyRow>> {
    sqlx::query_as("SELECT * FROM tenancy_companies ORDER BY created_at DESC LIMIT ?1 OFFSET ?2")
        .bind(limit)
        .bind(offset)
        .fetch_all(pool)
        .await
}
