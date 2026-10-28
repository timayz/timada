//! Queries over `automations_scenarios`: the automations page of a tenant.

use sqlx::{FromRow, SqlitePool};

use crate::value_object::ScenarioStatus;

/// One row of `automations_scenarios`.
#[derive(Debug, Clone, FromRow, PartialEq, Eq)]
pub struct ScenarioRow {
    pub tenant_id: String,
    pub id: String,
    pub name: String,
    pub status: String,
    /// The trigger's kind, once picked.
    pub trigger: Option<String>,
    /// The version running; 0 while never published.
    pub version: i64,
    /// Whether the draft holds what the version running does not.
    pub unpublished: bool,
    /// How many steps it holds, every lane counted.
    pub steps: i64,
    /// Unix seconds.
    pub created_at: i64,
    pub updated_at: i64,
}

impl ScenarioRow {
    pub fn status(&self) -> ScenarioStatus {
        ScenarioStatus::parse(&self.status).unwrap_or_default()
    }
}

/// Which scenarios a list shows: of one status, or every one not deleted.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScenarioFilter {
    pub status: Option<ScenarioStatus>,
}

impl ScenarioFilter {
    fn status(&self) -> Option<&'static str> {
        self.status.map(ScenarioStatus::as_str)
    }
}

/// The tenant's scenarios, last changed first.
pub async fn scenarios_of_tenant(
    pool: &SqlitePool,
    tenant_id: &str,
    filter: &ScenarioFilter,
    limit: u32,
    offset: u32,
) -> anyhow::Result<Vec<ScenarioRow>> {
    Ok(sqlx::query_as(
        "SELECT * FROM automations_scenarios
         WHERE tenant_id = ?1
           AND ((?2 IS NULL AND status <> 'archived') OR status = ?2)
         ORDER BY updated_at DESC, id DESC
         LIMIT ?3 OFFSET ?4",
    )
    .bind(tenant_id)
    .bind(filter.status())
    .bind(i64::from(limit))
    .bind(i64::from(offset))
    .fetch_all(pool)
    .await?)
}

/// How many scenarios the filter shows.
pub async fn count_scenarios(
    pool: &SqlitePool,
    tenant_id: &str,
    filter: &ScenarioFilter,
) -> anyhow::Result<u64> {
    let (count,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM automations_scenarios
         WHERE tenant_id = ?1
           AND ((?2 IS NULL AND status <> 'archived') OR status = ?2)",
    )
    .bind(tenant_id)
    .bind(filter.status())
    .fetch_one(pool)
    .await?;
    Ok(count as u64)
}
