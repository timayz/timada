//! Queries over `tenancy_api_keys`, for the API keys page of a tenant.

use sqlx::{FromRow, SqlitePool};

/// One row of `tenancy_api_keys`.
#[derive(Debug, Clone, FromRow, PartialEq, Eq)]
pub struct ApiKeyRow {
    pub id: String,
    pub tenant_id: String,
    pub label: String,
    pub hint: String,
    pub created_by: String,
    /// Unix seconds.
    pub created_at: i64,
    pub revoked_by: Option<String>,
    /// Unix seconds.
    pub revoked_at: Option<i64>,
}

impl ApiKeyRow {
    pub fn is_active(&self) -> bool {
        self.revoked_at.is_none()
    }
}

/// Every key of a tenant, revoked ones included, newest first.
pub async fn api_keys_of_tenant(
    pool: &SqlitePool,
    tenant_id: &str,
) -> sqlx::Result<Vec<ApiKeyRow>> {
    sqlx::query_as(
        "SELECT id, tenant_id, label, hint, created_by, created_at, revoked_by, revoked_at
         FROM tenancy_api_keys WHERE tenant_id = ?1
         ORDER BY created_at DESC, id",
    )
    .bind(tenant_id)
    .fetch_all(pool)
    .await
}
