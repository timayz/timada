//! Queries over `tenancy_companies`, for the companies pages.

use anyhow::Result;
use timada_core::db::{Db, FromRow};

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
pub async fn companies_of_user(db: &Db, user_id: &str) -> Result<Vec<CompanyRow>> {
    db.fetch_all(
        "SELECT c.id, c.siren, c.legal_name, c.created_at FROM tenancy_companies c
         JOIN tenancy_company_owners o ON o.company_id = c.id
         WHERE o.user_id = ?1 ORDER BY c.legal_name COLLATE NOCASE",
        (user_id,),
    )
    .await
}

/// The tenants selling as a company, by name.
pub async fn tenants_of_company(db: &Db, company_id: &str) -> Result<Vec<CompanyTenantRow>> {
    db.fetch_all(
        "SELECT id, slug, name, COALESCE(series_code, '') AS series_code FROM tenancy_tenants
         WHERE company_id = ?1 ORDER BY name COLLATE NOCASE",
        (company_id,),
    )
    .await
}

/// Every company, newest first, for the platform administrators.
pub async fn all_companies(db: &Db, offset: i64, limit: i64) -> Result<Vec<CompanyRow>> {
    db.fetch_all(
        "SELECT id, siren, legal_name, created_at FROM tenancy_companies
         ORDER BY created_at DESC LIMIT ?1 OFFSET ?2",
        (limit, offset),
    )
    .await
}
