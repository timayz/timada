//! Queries over `automations_versions`: every version of a scenario as it
//! was published, which the runs pinned to it walk and their pages show. A
//! version never changes once published, so a copy a moment behind is only
//! one that is not there yet.

use timada_core::db::Db;

/// The definition of a version, as JSON, as it was published; `None` for
/// one never published, or not seen here yet.
pub async fn published_definition(
    db: &Db,
    tenant_id: &str,
    scenario_id: &str,
    version: u32,
) -> anyhow::Result<Option<String>> {
    let found: Option<(String,)> = db
        .fetch_optional(
            "SELECT definition FROM automations_versions
             WHERE tenant_id = ?1 AND scenario_id = ?2 AND version = ?3",
            (tenant_id, scenario_id, i64::from(version)),
        )
        .await?;
    Ok(found.map(|(definition,)| definition))
}
