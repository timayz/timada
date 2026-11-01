//! Queries over `automations_scenarios`: the automations page of a tenant.

use timada_core::db::{Db, FromRow};

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
    /// How many runs it made (still listed).
    pub runs: i64,
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
    db: &Db,
    tenant_id: &str,
    filter: &ScenarioFilter,
    limit: u32,
    offset: u32,
) -> anyhow::Result<Vec<ScenarioRow>> {
    db.fetch_all(
        "SELECT s.*,
            (SELECT COUNT(*) FROM automations_runs r
             WHERE r.tenant_id = s.tenant_id AND r.scenario_id = s.id) AS runs
         FROM automations_scenarios s
         WHERE s.tenant_id = ?1
           AND ((?2 IS NULL AND s.status <> 'archived') OR s.status = ?2)
         ORDER BY s.updated_at DESC, s.id DESC
         LIMIT ?3 OFFSET ?4",
        (
            tenant_id,
            filter.status(),
            i64::from(limit),
            i64::from(offset),
        ),
    )
    .await
}

/// How many scenarios the filter shows.
pub async fn count_scenarios(
    db: &Db,
    tenant_id: &str,
    filter: &ScenarioFilter,
) -> anyhow::Result<u64> {
    let (count,): (i64,) = db
        .fetch_one(
            "SELECT COUNT(*) FROM automations_scenarios
         WHERE tenant_id = ?1
           AND ((?2 IS NULL AND status <> 'archived') OR status = ?2)",
            (tenant_id, filter.status()),
        )
        .await?;
    Ok(count as u64)
}

/// The name of one of the tenant's scenarios.
pub async fn scenario_name(
    db: &Db,
    tenant_id: &str,
    scenario_id: &str,
) -> anyhow::Result<Option<String>> {
    let row: Option<(String,)> = db
        .fetch_optional(
            "SELECT name FROM automations_scenarios WHERE tenant_id = ?1 AND id = ?2",
            (tenant_id, scenario_id),
        )
        .await?;
    Ok(row.map(|(name,)| name))
}
