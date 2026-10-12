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
    /// [`group_of`](crate::group_of) of `id`, stored as a signed 64-bit.
    pub group_id: i64,
    pub member_count: i64,
}

impl TenantRow {
    pub fn group(&self) -> u64 {
        self.group_id as u64
    }
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

/// A `LIKE` pattern matching `q` anywhere, with its wildcards escaped.
fn contains(q: &str) -> String {
    let escaped = q
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_");
    format!("%{escaped}%")
}

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

/// Tenants whose slug or name contains `q` (every tenant when `None`), by name.
pub async fn list_tenants(
    pool: &SqlitePool,
    q: Option<&str>,
    offset: i64,
    limit: i64,
) -> sqlx::Result<Vec<TenantRow>> {
    let pattern = q.map(contains);
    sqlx::query_as(
        "SELECT id, slug, name, created_by, created_at, suspended, suspended_reason, group_id,
            member_count
         FROM tenancy_tenants
         WHERE ?1 IS NULL OR slug LIKE ?1 ESCAPE '\\' OR name LIKE ?1 ESCAPE '\\'
         ORDER BY name COLLATE NOCASE, id
         LIMIT ?2 OFFSET ?3",
    )
    .bind(pattern)
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await
}

pub async fn count_tenants(pool: &SqlitePool, q: Option<&str>) -> sqlx::Result<i64> {
    let pattern = q.map(contains);
    sqlx::query_scalar(
        "SELECT COUNT(*) FROM tenancy_tenants
         WHERE ?1 IS NULL OR slug LIKE ?1 ESCAPE '\\' OR name LIKE ?1 ESCAPE '\\'",
    )
    .bind(pattern)
    .fetch_one(pool)
    .await
}

/// The tenant whose database runs in consensus group `group`, if the catalog
/// knows one.
pub async fn tenant_by_group(pool: &SqlitePool, group: u64) -> sqlx::Result<Option<TenantRow>> {
    sqlx::query_as(
        "SELECT id, slug, name, created_by, created_at, suspended, suspended_reason, group_id,
            member_count
         FROM tenancy_tenants WHERE group_id = ?1",
    )
    .bind(group as i64)
    .fetch_optional(pool)
    .await
}

#[cfg(test)]
mod tests {
    #[test]
    fn like_patterns_escape_wildcards() {
        assert_eq!(super::contains("a_b%c\\"), "%a\\_b\\%c\\\\%");
    }
}
