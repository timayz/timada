//! Queries over the read models, for the home page (a user's tenants) and
//! the administration pages.

use anyhow::Result;
use timada_core::db::{Db, FromRow};

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

use timada_core::db::fts_query as phrase;
use timada_core::like_contains as contains;

/// The tenants `user_id` is a member of, by name.
pub async fn tenants_of_user(db: &Db, user_id: &str) -> Result<Vec<MembershipRow>> {
    db.fetch_all(
        "SELECT t.id AS tenant_id, t.slug, t.name, t.suspended, m.role
         FROM tenancy_members m JOIN tenancy_tenants t ON t.id = m.tenant_id
         WHERE m.user_id = ?1
         ORDER BY t.name COLLATE NOCASE, t.id",
        (user_id,),
    )
    .await
}

/// Where the tenants come from, `?1` being what was searched for. With a
/// phrase the full-text index narrows the rows first; without one `?1` is a
/// `LIKE` pattern or `NULL`.
macro_rules! tenants_from {
    (phrase) => {
        "FROM tenancy_tenants t
         WHERE t.id IN (SELECT id FROM tenancy_tenants
                        WHERE fts_match(slug_search, name_search, ?1)) "
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
    db: &Db,
    q: Option<&str>,
    offset: i64,
    limit: i64,
) -> Result<Vec<TenantRow>> {
    let (search, indexed) = search_of(q);
    let statement = if indexed {
        tenants_page!(phrase)
    } else {
        tenants_page!(pattern)
    };
    db.fetch_all(statement, (search, limit, offset)).await
}

/// How many tenants [`list_tenants`] would list: with no search, the total
/// the subscription keeps; with one, the matches counted.
pub async fn count_tenants(db: &Db, q: Option<&str>) -> Result<i64> {
    if q.is_none() {
        return db
            .fetch_scalar("SELECT tenants FROM tenancy_totals WHERE id = 1", ())
            .await;
    }
    let (search, indexed) = search_of(q);
    let statement = if indexed {
        concat!("SELECT COUNT(*) ", tenants_from!(phrase))
    } else {
        concat!("SELECT COUNT(*) ", tenants_from!(pattern))
    };
    db.fetch_scalar(statement, (search,)).await
}

/// The ids of every tenant not suspended, oldest first: the tenants a node
/// keeps a copy of.
pub async fn active_tenant_ids(db: &Db) -> Result<Vec<String>> {
    let rows: Vec<(String,)> = db
        .fetch_all(
            "SELECT id FROM tenancy_tenants WHERE suspended = 0 ORDER BY created_at, id",
            (),
        )
        .await?;
    Ok(rows.into_iter().map(|(id,)| id).collect())
}

#[cfg(test)]
mod tests {
    #[test]
    fn like_patterns_escape_wildcards() {
        assert_eq!(super::contains("a_b%c\\"), "%a\\_b\\%c\\\\%");
    }
}
