//! The scheduled exports due, for the sweep that sends them.

use timada_core::db::{Db, FromRow};

/// A scheduled export to look at: its shop and what it exports (`orders`,
/// `customers`, `newsletter`).
#[derive(Debug, Clone, PartialEq, Eq, FromRow)]
pub struct ExportDue {
    pub tenant_id: String,
    pub what: String,
    /// Unix seconds.
    pub due_at: i64,
}

/// The scheduled exports, of every tenant, whose time came at `now` (unix
/// seconds), the longest due first, from the one after `after` on: read
/// off the index on `due_at`, a page at a time, so a schedule that stays
/// due (its shop is suspended) holds no other back. The write model
/// decides whether one is sent.
pub async fn export_schedules_due(
    db: &Db,
    now: i64,
    after: Option<&ExportDue>,
    limit: i64,
) -> anyhow::Result<Vec<ExportDue>> {
    let (at, tenant_id, what) = match after {
        Some(after) => (after.due_at, after.tenant_id.as_str(), after.what.as_str()),
        None => (i64::MIN, "", ""),
    };
    db.fetch_all(
        "SELECT tenant_id, what, due_at FROM orders_export_schedules
         WHERE due_at <= ?1
           AND (due_at > ?2 OR (due_at = ?2
                AND (tenant_id > ?3 OR (tenant_id = ?3 AND what > ?4))))
         ORDER BY due_at, tenant_id, what LIMIT ?5",
        (now, at, tenant_id, what, limit),
    )
    .await
}
