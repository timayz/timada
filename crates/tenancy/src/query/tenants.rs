//! Queries over the read models, for the home page (a user's tenants) and
//! the administration pages.

use sqlx::{FromRow, SqlitePool};

/// One row of `tenancy_tenants`.
#[derive(Debug, Clone, FromRow, PartialEq, Eq)]
pub struct TenantRow {
    pub id: String,
    pub slug: String,
    pub name: String,
    pub created_by: String,
    /// Unix seconds.
    pub created_at: i64,
    pub suspended: bool,
    pub suspended_reason: Option<String>,
    pub member_count: i64,
}

/// A tenant a user belongs to, with the role they hold there.
#[derive(Debug, Clone, FromRow, PartialEq, Eq)]
pub struct MembershipRow {
    pub tenant_id: String,
    pub slug: String,
    pub name: String,
    pub suspended: bool,
    pub role: String,
}

use timada_core::like_contains as contains;

/// The tenants `user_id` is a member of, by name.
pub async fn tenants_of_user(pool: &SqlitePool, user_id: &str) -> sqlx::Result<Vec<MembershipRow>> {
    sqlx::query_as(
        "SELECT t.id AS tenant_id, t.slug, t.name, t.suspended, m.role
         FROM tenancy_members m JOIN tenancy_tenants t ON t.id = m.tenant_id
         WHERE m.user_id = ?1
         ORDER BY t.name COLLATE NOCASE, t.id",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await
}

/// The full-text phrase matching `q` anywhere in a word, when `q` is long
/// enough for the index (it is cut in runs of three characters); `None`
/// otherwise, and the search falls back to [`contains`].
fn phrase(q: &str) -> Option<String> {
    (q.chars().count() >= 3).then(|| format!("\"{}\"", q.replace('"', "\"\"")))
}

/// Where the tenants come from, `?1` being what was searched for. With a
/// phrase the full-text index leads (`CROSS JOIN` keeps the planner from
/// walking every tenant in order instead); without one `?1` is a `LIKE`
/// pattern or `NULL`.
macro_rules! tenants_from {
    (phrase) => {
        "FROM tenancy_tenants_fts f CROSS JOIN tenancy_tenants t ON t.rowid = f.rowid
         WHERE tenancy_tenants_fts MATCH ?1 "
    };
    (pattern) => {
        "FROM tenancy_tenants t
         WHERE ?1 IS NULL OR t.slug LIKE ?1 ESCAPE '\\' OR t.name LIKE ?1 ESCAPE '\\' "
    };
}

macro_rules! tenants_page {
    ($from:ident) => {
        concat!(
            "SELECT t.id, t.slug, t.name, t.created_by, t.created_at, t.suspended,
                t.suspended_reason, t.member_count ",
            tenants_from!($from),
            "ORDER BY t.name COLLATE NOCASE, t.id LIMIT ?2 OFFSET ?3"
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

/// Tenants whose slug or name contains `q` (every tenant when `None`), by name.
pub async fn list_tenants(
    pool: &SqlitePool,
    q: Option<&str>,
    offset: i64,
    limit: i64,
) -> sqlx::Result<Vec<TenantRow>> {
    let (search, indexed) = search_of(q);
    let statement = if indexed {
        tenants_page!(phrase)
    } else {
        tenants_page!(pattern)
    };
    sqlx::query_as(statement)
        .bind(search)
        .bind(limit)
        .bind(offset)
        .fetch_all(pool)
        .await
}

/// How many tenants [`list_tenants`] would list: with no search, the total
/// the subscription keeps; with one, the matches counted.
pub async fn count_tenants(pool: &SqlitePool, q: Option<&str>) -> sqlx::Result<i64> {
    if q.is_none() {
        return sqlx::query_scalar("SELECT tenants FROM tenancy_totals WHERE id = 1")
            .fetch_one(pool)
            .await;
    }
    let (search, indexed) = search_of(q);
    let statement = if indexed {
        concat!("SELECT COUNT(*) ", tenants_from!(phrase))
    } else {
        concat!("SELECT COUNT(*) ", tenants_from!(pattern))
    };
    sqlx::query_scalar(statement)
        .bind(search)
        .fetch_one(pool)
        .await
}

/// The ids of every tenant not suspended, oldest first: the tenants a node
/// keeps a copy of.
pub async fn active_tenant_ids(pool: &SqlitePool) -> sqlx::Result<Vec<String>> {
    sqlx::query_scalar("SELECT id FROM tenancy_tenants WHERE suspended = 0 ORDER BY created_at, id")
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
