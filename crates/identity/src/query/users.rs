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
    /// The address of the e-mail + password method, when the user has one.
    pub email: Option<String>,
    pub has_password: bool,
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

/// The full-text phrase matching `q` anywhere in a word, when `q` is long
/// enough for the index (it is cut in runs of three characters); `None`
/// otherwise, and the search falls back to [`contains`].
fn phrase(q: &str) -> Option<String> {
    (q.chars().count() >= 3).then(|| format!("\"{}\"", q.replace('"', "\"\"")))
}

/// Where the users come from, `?1` being what was searched for. With a phrase
/// the full-text index leads (`CROSS JOIN` keeps the planner from walking
/// every user in order instead); without one `?1` is a `LIKE` pattern or
/// `NULL`.
macro_rules! users_from {
    (phrase) => {
        "FROM identity_users_fts f CROSS JOIN identity_users u ON u.rowid = f.rowid
         WHERE identity_users_fts MATCH ?1 "
    };
    (pattern) => {
        "FROM identity_users u
         WHERE ?1 IS NULL OR u.account LIKE ?1 ESCAPE '\\' OR u.label LIKE ?1 ESCAPE '\\' "
    };
}

macro_rules! users_page {
    ($from:ident) => {
        concat!(
            "SELECT u.id, u.account, u.label, u.is_admin, u.deleted, u.registered_at,
                u.last_sign_in_at, u.sign_in_count, u.oauth2_count, u.passkey_count, u.email,
                u.has_password ",
            users_from!($from),
            "ORDER BY u.deleted, u.label COLLATE NOCASE, u.id LIMIT ?2 OFFSET ?3"
        )
    };
}

/// What `?1` is, and whether it is a phrase for the index.
fn search_of(q: Option<&str>) -> (Option<String>, bool) {
    match q.and_then(phrase) {
        Some(phrase) => (Some(phrase), true),
        None => (q.map(contains), false),
    }
}

/// Users whose account or name contains `q` (everyone when `None`),
/// deleted ones last, then by name.
pub async fn list_users(
    pool: &SqlitePool,
    q: Option<&str>,
    offset: i64,
    limit: i64,
) -> sqlx::Result<Vec<UserRow>> {
    let (search, indexed) = search_of(q);
    let statement = if indexed {
        users_page!(phrase)
    } else {
        users_page!(pattern)
    };
    sqlx::query_as(statement)
        .bind(search)
        .bind(limit)
        .bind(offset)
        .fetch_all(pool)
        .await
}

pub async fn count_users(pool: &SqlitePool, q: Option<&str>) -> sqlx::Result<i64> {
    let (search, indexed) = search_of(q);
    let statement = if indexed {
        concat!("SELECT COUNT(*) ", users_from!(phrase))
    } else {
        concat!("SELECT COUNT(*) ", users_from!(pattern))
    };
    sqlx::query_scalar(statement)
        .bind(search)
        .fetch_one(pool)
        .await
}

/// The rows of the given users, in no particular order; unknown ids are
/// skipped. For pages that show other contexts' user ids by name.
pub async fn users_by_ids(pool: &SqlitePool, ids: &[String]) -> sqlx::Result<Vec<UserRow>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let mut query = sqlx::QueryBuilder::new(
        "SELECT id, account, label, is_admin, deleted, registered_at, last_sign_in_at,
            sign_in_count, oauth2_count, passkey_count, email, has_password
         FROM identity_users WHERE id IN (",
    );
    let mut separated = query.separated(", ");
    for id in ids {
        separated.push_bind(id);
    }
    separated.push_unseparated(")");
    query.build_query_as().fetch_all(pool).await
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
