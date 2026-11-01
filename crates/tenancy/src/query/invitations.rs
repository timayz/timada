//! Queries over `tenancy_invitations`, for the members page of a tenant.

use anyhow::Result;
use timada_core::db::{Db, FromRow};

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
    db: &Db,
    tenant_id: &str,
    now: u64,
) -> Result<Vec<InvitationRow>> {
    db.fetch_all(
        "SELECT id, tenant_id, email, role, invited_by, sent_at, expires_at, accepted_by,
            accepted_at, revoked
         FROM tenancy_invitations
         WHERE tenant_id = ?1 AND accepted_by IS NULL AND revoked = 0 AND expires_at > ?2
         ORDER BY sent_at DESC, id",
        (tenant_id, now as i64),
    )
    .await
}

/// How many invitations of a tenant are still waiting at `now`: the rows
/// [`open_invitations_of_tenant`] would return.
pub async fn count_open_invitations_of_tenant(db: &Db, tenant_id: &str, now: u64) -> Result<i64> {
    db.fetch_scalar(
        "SELECT COUNT(*) FROM tenancy_invitations
         WHERE tenant_id = ?1 AND accepted_by IS NULL AND revoked = 0 AND expires_at > ?2",
        (tenant_id, now as i64),
    )
    .await
}

/// How many invitations were sent since a moment, by a tenant and by an
/// account (across every tenant). Accepted, revoked and expired ones count
/// too: each was a mail.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SentCounts {
    pub tenant: i64,
    pub account: i64,
}

/// The invitations `tenant_id` sent, and those `user_id` sent, at or after
/// `since` (unix seconds).
pub async fn invitations_sent_since(
    db: &Db,
    tenant_id: &str,
    user_id: &str,
    since: u64,
) -> Result<SentCounts> {
    let (tenant, account): (i64, i64) = db
        .fetch_one(
            "SELECT
                (SELECT COUNT(*) FROM tenancy_invitations WHERE tenant_id = ?1 AND sent_at >= ?3),
                (SELECT COUNT(*) FROM tenancy_invitations WHERE invited_by = ?2 AND sent_at >= ?3)",
            (tenant_id, user_id, since as i64),
        )
        .await?;
    Ok(SentCounts { tenant, account })
}

pub async fn invitation_by_id(db: &Db, id: &str) -> Result<Option<InvitationRow>> {
    db.fetch_optional(
        "SELECT id, tenant_id, email, role, invited_by, sent_at, expires_at, accepted_by,
            accepted_at, revoked
         FROM tenancy_invitations WHERE id = ?1",
        (id,),
    )
    .await
}
