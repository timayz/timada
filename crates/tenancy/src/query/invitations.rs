//! Queries over `tenancy_invitations`, for the members page of a tenant.

use sqlx::{FromRow, SqlitePool};

/// One row of `tenancy_invitations`.
#[derive(Debug, Clone, FromRow, PartialEq, Eq)]
pub struct InvitationRow {
    pub id: String,
    pub tenant_id: String,
    pub email: String,
    pub role: String,
    pub invited_by: String,
    /// Unix seconds.
    pub sent_at: i64,
    /// Unix seconds.
    pub expires_at: i64,
    pub accepted_by: Option<String>,
    pub accepted_at: Option<i64>,
    pub revoked: bool,
}

/// The invitations of a tenant still waiting at `now`: not accepted, not
/// revoked, not expired; newest first.
pub async fn open_invitations_of_tenant(
    pool: &SqlitePool,
    tenant_id: &str,
    now: u64,
) -> sqlx::Result<Vec<InvitationRow>> {
    sqlx::query_as(
        "SELECT id, tenant_id, email, role, invited_by, sent_at, expires_at, accepted_by,
            accepted_at, revoked
         FROM tenancy_invitations
         WHERE tenant_id = ?1 AND accepted_by IS NULL AND revoked = 0 AND expires_at > ?2
         ORDER BY sent_at DESC, id",
    )
    .bind(tenant_id)
    .bind(now as i64)
    .fetch_all(pool)
    .await
}

/// How many invitations of a tenant are still waiting at `now`: the rows
/// [`open_invitations_of_tenant`] would return.
pub async fn count_open_invitations_of_tenant(
    pool: &SqlitePool,
    tenant_id: &str,
    now: u64,
) -> sqlx::Result<i64> {
    sqlx::query_scalar(
        "SELECT COUNT(*) FROM tenancy_invitations
         WHERE tenant_id = ?1 AND accepted_by IS NULL AND revoked = 0 AND expires_at > ?2",
    )
    .bind(tenant_id)
    .bind(now as i64)
    .fetch_one(pool)
    .await
}

pub async fn invitation_by_id(pool: &SqlitePool, id: &str) -> sqlx::Result<Option<InvitationRow>> {
    sqlx::query_as(
        "SELECT id, tenant_id, email, role, invited_by, sent_at, expires_at, accepted_by,
            accepted_at, revoked
         FROM tenancy_invitations WHERE id = ?1",
    )
    .bind(id)
    .fetch_optional(pool)
    .await
}
