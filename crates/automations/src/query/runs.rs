//! Queries over `automations_runs`: a scenario's run history, and the runs
//! the runner owes a look.

use timada_core::Share;
use timada_core::db::{Db, FromRow};

use crate::value_object::RunStatus;

/// One row of `automations_runs`.
#[derive(Debug, Clone, FromRow, PartialEq, Eq)]
pub struct RunRow {
    pub tenant_id: String,
    pub id: String,
    pub scenario_id: String,
    pub scenario_version: i64,
    pub trigger: String,
    pub subject_id: String,
    pub status: String,
    /// The step it is at, or ended at.
    pub step: Option<String>,
    /// Why it failed or was cancelled.
    pub reason: Option<String>,
    /// Unix seconds when the runner looks at it again; none once it ended.
    pub next_at: Option<i64>,
    pub started_at: i64,
    pub updated_at: i64,
}

impl RunRow {
    pub fn status(&self) -> RunStatus {
        RunStatus::parse(&self.status).unwrap_or_default()
    }
}

/// Which runs a list shows: of one status, or all.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RunFilter {
    pub status: Option<RunStatus>,
}

/// A scenario's runs, newest first.
pub async fn runs_of_scenario(
    db: &Db,
    tenant_id: &str,
    scenario_id: &str,
    filter: &RunFilter,
    limit: u32,
    offset: u32,
) -> anyhow::Result<Vec<RunRow>> {
    db.fetch_all(
        "SELECT * FROM automations_runs
         WHERE tenant_id = ?1 AND scenario_id = ?2 AND (?3 IS NULL OR status = ?3)
         ORDER BY started_at DESC, id DESC
         LIMIT ?4 OFFSET ?5",
        (
            tenant_id,
            scenario_id,
            filter.status.map(RunStatus::as_str),
            i64::from(limit),
            i64::from(offset),
        ),
    )
    .await
}

/// How many runs [`runs_of_scenario`] would list.
pub async fn count_runs(
    db: &Db,
    tenant_id: &str,
    scenario_id: &str,
    filter: &RunFilter,
) -> anyhow::Result<u64> {
    let (count,): (i64,) = db
        .fetch_one(
            "SELECT COUNT(*) FROM automations_runs
         WHERE tenant_id = ?1 AND scenario_id = ?2 AND (?3 IS NULL OR status = ?3)",
            (tenant_id, scenario_id, filter.status.map(RunStatus::as_str)),
        )
        .await?;
    Ok(count as u64)
}

/// What [`due_runs`] reads: off the index of the runs due, in its order.
pub(super) const DUE_RUNS: &str =
    "SELECT tenant_id, id FROM automations_runs INDEXED BY automations_runs_due
     WHERE next_at IS NOT NULL AND next_at <= ?1
       AND (shard % ?2 = ?3 OR next_at <= ?4)
     ORDER BY next_at, id
     LIMIT ?5";

/// The runs due by `now`, every tenant's, that a node with `share` moves:
/// its own part of them, and any due at `help_before` or earlier, which
/// their own node has not moved (it stopped, or the nodes disagreed for a
/// moment on who is up). `(tenant id, run id)`, the most overdue first. The
/// read model is a moment behind: a run moved on a moment ago may come
/// back, and the runner then finds nothing to do.
pub async fn due_runs(
    db: &Db,
    now: u64,
    share: Share,
    help_before: u64,
    limit: u32,
) -> anyhow::Result<Vec<(String, String)>> {
    db.fetch_all(
        DUE_RUNS,
        (
            now as i64,
            share.of as i64,
            share.index as i64,
            help_before as i64,
            i64::from(limit),
        ),
    )
    .await
}

/// Whether the read model holds a run: it was started, by whichever node.
pub async fn run_known(db: &Db, tenant_id: &str, run_id: &str) -> anyhow::Result<bool> {
    let found: Option<(i64,)> = db
        .fetch_optional(
            "SELECT 1 FROM automations_runs WHERE tenant_id = ?1 AND id = ?2",
            (tenant_id, run_id),
        )
        .await?;
    Ok(found.is_some())
}

/// Drops the rows of runs that ended before `before` (the store keeps
/// them); how many.
pub async fn prune_runs(db: &Db, before: u64) -> anyhow::Result<u64> {
    let done = db
        .execute(
            "DELETE FROM automations_runs WHERE next_at IS NULL AND updated_at < ?1",
            (before as i64,),
        )
        .await?;
    Ok(done)
}
