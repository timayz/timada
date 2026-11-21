//! Queries over `identity_invitations`, for the administrators' page of
//! users.

use anyhow::Result;
use timada_core::MailStatus;
use timada_core::db::{Db, FromRow};

/// One row of `identity_invitations`.
#[derive(Debug, Clone, FromRow, PartialEq, Eq)]
pub struct AccountInvitationRow {
    pub id: String,
    pub email: String,
    pub invited_by: String,
    /// Unix seconds.
    pub sent_at: i64,
    /// Unix seconds.
    pub expires_at: i64,
    pub accepted_by: Option<String>,
    pub accepted_at: Option<i64>,
    pub revoked: bool,
    /// Where the link's mail stands ([`MailStatus::as_str`]).
    pub mail: String,
}

impl AccountInvitationRow {
    pub fn mail(&self) -> MailStatus {
        MailStatus::parse(&self.mail)
    }
}

/// The most invitations the page lists: root sends them one at a time.
pub const OPEN_ACCOUNT_INVITATIONS_SHOWN: i64 = 200;

/// The invitations to create an account still waiting at `now`: not used,
/// not revoked, not expired; newest first, at most
/// [`OPEN_ACCOUNT_INVITATIONS_SHOWN`].
pub async fn open_account_invitations(db: &Db, now: u64) -> Result<Vec<AccountInvitationRow>> {
    db.fetch_all(
        "SELECT id, email, invited_by, sent_at, expires_at, accepted_by, accepted_at, revoked,
            mail
         FROM identity_invitations
         WHERE revoked = 0 AND expires_at > ?1 AND accepted_by IS NULL
         ORDER BY sent_at DESC, id LIMIT ?2",
        (now as i64, OPEN_ACCOUNT_INVITATIONS_SHOWN),
    )
    .await
}
