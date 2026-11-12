//! Queries over `automations_runs`: a scenario's run history, and the runs
//! the runner owes a look, which it puts off when it leaves one as it was.

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
    /// Unix seconds when this node's runner looks at it again (later than
    /// the run says when the runner left it as it was); none once it ended.
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

/// A run [`due_runs`] lists: where a walk of the runs due stands once it
/// read it.
#[derive(Debug, Clone, FromRow, PartialEq, Eq)]
pub struct DueRun {
    pub tenant_id: String,
    pub id: String,
    /// Unix seconds: when this node is to look at it, as its row says.
    pub next_at: i64,
}

/// What [`due_runs`] reads: off the index of the runs due, in its order,
/// from where the walk stands (the bound on `next_at` alone lets it start
/// there rather than walk up to it).
pub(super) const DUE_RUNS: &str =
    "SELECT tenant_id, id, next_at FROM automations_runs INDEXED BY automations_runs_due
     WHERE next_at IS NOT NULL AND paused = 0
       AND next_at >= ?5 AND (next_at > ?5 OR id > ?6) AND next_at <= ?1
       AND (shard % ?2 = ?3 OR next_at <= ?4)
     ORDER BY next_at, id
     LIMIT ?7";

/// The runs due by `now`, every tenant's, that a node with `share` moves:
/// its own part of them, and any due at `help_before` or earlier, which
/// their own node has not moved (it stopped, or the nodes disagreed for a
/// moment on who is up). The most overdue first, after `after` (the last
/// run the walk read) when given. A paused scenario's runs are not due
/// until it goes on. The read model is a moment behind: a run moved on a
/// moment ago may come back, and the runner then finds nothing to do.
pub async fn due_runs(
    db: &Db,
    now: u64,
    share: Share,
    help_before: u64,
    after: Option<&DueRun>,
    limit: u32,
) -> anyhow::Result<Vec<DueRun>> {
    let (from, from_id) = after.map_or((i64::MIN, ""), |run| (run.next_at, run.id.as_str()));
    db.fetch_all(
        DUE_RUNS,
        (
            now as i64,
            share.of as i64,
            share.index as i64,
            help_before as i64,
            from,
            from_id,
            i64::from(limit),
        ),
    )
    .await
}

/// What [`put_off_run`] writes, by the table's key: the planner does not
/// pick it by itself, as the index of a scenario's runs leads with the
/// tenant too.
pub(super) const PUT_OFF_RUN: &str =
    "UPDATE automations_runs INDEXED BY sqlite_autoindex_automations_runs_1 SET next_at = ?4
     WHERE tenant_id = ?1 AND id = ?2 AND next_at = ?3";

/// Puts this node's next look at a run its walk read (`run`) off to
/// `until`: the runner left it as it was. The row is this node's own: any
/// move of the run, by any node, writes it again, and a row written since
/// the walk read it is left as it is.
pub async fn put_off_run(db: &Db, run: &DueRun, until: u64) -> anyhow::Result<()> {
    db.execute(
        PUT_OFF_RUN,
        (
            run.tenant_id.as_str(),
            run.id.as_str(),
            run.next_at,
            until as i64,
        ),
    )
    .await?;
    Ok(())
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
