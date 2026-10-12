//! Queries over the read models, for the administration pages and the
//! sessions list of the account page.

use sqlx::{FromRow, SqlitePool};

/// One row of `identity_users`.
#[derive(Debug, Clone, FromRow, PartialEq, Eq)]
pub struct UserRow {
    pub id: String,
    pub account: String,
    pub label: String,
    pub is_admin: bool,
    pub deleted: bool,
    /// Unix seconds.
    pub registered_at: i64,
    /// Unix seconds.
    pub last_sign_in_at: Option<i64>,
    pub sign_in_count: i64,
    pub oauth2_count: i64,
    pub passkey_count: i64,
}

/// One row of `identity_sessions`.
#[derive(Debug, Clone, FromRow, PartialEq, Eq)]
pub struct SessionRow {
    pub id: String,
    pub user_id: String,
    /// Unix seconds.
    pub started_at: i64,
    /// Unix seconds.
    pub expires_at: i64,
    pub ended: bool,
    pub user_agent: Option<String>,
    pub ip: Option<String>,
}

impl SessionRow {
    pub fn is_active_at(&self, now: i64) -> bool {
        !self.ended && self.expires_at > now
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

/// Users whose account or name contains `q` (everyone when `None`),
/// deleted ones last, then by name.
pub async fn list_users(
    pool: &SqlitePool,
    q: Option<&str>,
    offset: i64,
    limit: i64,
) -> sqlx::Result<Vec<UserRow>> {
    let pattern = q.map(contains);
    sqlx::query_as(
        "SELECT id, account, label, is_admin, deleted, registered_at, last_sign_in_at,
            sign_in_count, oauth2_count, passkey_count
         FROM identity_users
         WHERE ?1 IS NULL OR account LIKE ?1 ESCAPE '\\' OR label LIKE ?1 ESCAPE '\\'
         ORDER BY deleted, label COLLATE NOCASE, id
         LIMIT ?2 OFFSET ?3",
    )
    .bind(pattern)
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await
}

pub async fn count_users(pool: &SqlitePool, q: Option<&str>) -> sqlx::Result<i64> {
    let pattern = q.map(contains);
    sqlx::query_scalar(
        "SELECT COUNT(*) FROM identity_users
         WHERE ?1 IS NULL OR account LIKE ?1 ESCAPE '\\' OR label LIKE ?1 ESCAPE '\\'",
    )
    .bind(pattern)
    .fetch_one(pool)
    .await
}

/// Every known session of a user, newest first.
pub async fn user_sessions(pool: &SqlitePool, user_id: &str) -> sqlx::Result<Vec<SessionRow>> {
    sqlx::query_as(
        "SELECT id, user_id, started_at, expires_at, ended, user_agent, ip
         FROM identity_sessions WHERE user_id = ?1
         ORDER BY started_at DESC, id",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await
}

/// The sessions of a user that still authenticate at `now` (unix seconds),
/// newest first.
pub async fn active_sessions(
    pool: &SqlitePool,
    user_id: &str,
    now: i64,
) -> sqlx::Result<Vec<SessionRow>> {
    sqlx::query_as(
        "SELECT id, user_id, started_at, expires_at, ended, user_agent, ip
         FROM identity_sessions
         WHERE user_id = ?1 AND ended = 0 AND expires_at > ?2
         ORDER BY started_at DESC, id",
    )
    .bind(user_id)
    .bind(now)
    .fetch_all(pool)
    .await
}

#[cfg(test)]
mod tests {
    #[test]
    fn like_patterns_escape_wildcards() {
        assert_eq!(super::contains("a_b%c\\"), "%a\\_b\\%c\\\\%");
    }
}
