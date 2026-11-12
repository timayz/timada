//! Queries over `automations_runs`: a scenario's run history, with the
//! counts `automations_run_counts` keeps of it, and the runs the runner
//! owes a look, which it puts off when it leaves one as it was.

use timada_core::Share;
use timada_core::db::{Connection, Db, FromRow, execute, fetch_all, fetch_optional};
use timada_core::page::{Cursor, Page, PageFrom, Side, read_page};

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

/// The columns of [`RunRow`].
const RUN_COLUMNS: &str = "SELECT tenant_id, id, scenario_id, scenario_version, trigger,
    subject_id, status, step, reason, next_at, started_at, updated_at";

/// A page of a scenario's (`?2`) runs on either side of `(?4, ?5)`
/// (`started_at`, `id`), `?6` rows at most: walking down from it (`older`),
/// as the list reads, or up, nearest first; of the status `?3` when the
/// filter names one, which is written `?3 IS NULL` otherwise so the
/// numbering holds (written `?3 IS NULL OR status = ?3`, it hid the
/// indexes). Either way off the index of the scenario's runs (of that
/// status), in its order, so a page reads its rows and no others. The index
/// is named: left to choose, the planner read every run of the tenant off
/// the table's key, whose first column is the tenant too, and sorted them.
pub(super) fn page_statement(filter: &RunFilter, older: bool) -> String {
    let (index, status) = match filter.status {
        Some(_) => ("automations_runs_scenario_status", "status = ?3"),
        None => ("automations_runs_scenario", "?3 IS NULL"),
    };
    let tail = if older {
        "started_at <= ?4 AND (started_at < ?4 OR id < ?5)
         ORDER BY started_at DESC, id DESC LIMIT ?6"
    } else {
        "started_at >= ?4 AND (started_at > ?4 OR id > ?5)
         ORDER BY started_at ASC, id ASC LIMIT ?6"
    };
    format!(
        "{RUN_COLUMNS} FROM automations_runs INDEXED BY {index}
         WHERE tenant_id = ?1 AND scenario_id = ?2 AND {status} AND {tail}"
    )
}

/// Where a run stands in the list: when it started, and its id.
fn cursor_of(row: &RunRow) -> Cursor {
    Cursor {
        at: row.started_at,
        id: row.id.clone(),
    }
}

/// A page of a scenario's runs the filter matches, newest first, `limit`
/// rows at most (see [`timada_core::page`]).
pub async fn runs_of_scenario(
    db: &Db,
    tenant_id: &str,
    scenario_id: &str,
    filter: &RunFilter,
    from: PageFrom<'_>,
    limit: i64,
) -> anyhow::Result<Page<RunRow>> {
    let older = page_statement(filter, true);
    let newer = page_statement(filter, false);
    let status = filter.status.map(RunStatus::as_str);
    let read = |side: Side, cursor: Option<Cursor>, n: i64| {
        let statement = match side {
            Side::Older => older.as_str(),
            Side::Newer => newer.as_str(),
        };
        let (at, id) = cursor
            .map(|cursor| (cursor.at, cursor.id))
            .unwrap_or((i64::MAX, String::new()));
        db.fetch_all::<RunRow>(
            statement,
            (
                tenant_id.to_owned(),
                scenario_id.to_owned(),
                status,
                at,
                id,
                n,
            ),
        )
    };
    read_page(from, limit, read, cursor_of).await
}

/// What [`count_runs`] reads for the filter: the counts kept of the
/// scenario's (`?2`) runs listed, of the status `?3` when the filter names
/// one (`?3 IS NULL` otherwise, as in [`page_statement`]), by their key.
pub(super) fn count_statement(filter: &RunFilter) -> String {
    let status = match filter.status {
        Some(_) => "status = ?3",
        None => "?3 IS NULL",
    };
    format!(
        "SELECT COALESCE(SUM(runs), 0) FROM automations_run_counts
         WHERE tenant_id = ?1 AND scenario_id = ?2 AND {status}"
    )
}

/// How many runs [`runs_of_scenario`] lists over all its pages, as the read
/// model counts them while it writes their rows.
pub async fn count_runs(
    db: &Db,
    tenant_id: &str,
    scenario_id: &str,
    filter: &RunFilter,
) -> anyhow::Result<u64> {
    let count: i64 = db
        .fetch_scalar(
            &count_statement(filter),
            (tenant_id, scenario_id, filter.status.map(RunStatus::as_str)),
        )
        .await?;
    Ok(count.max(0) as u64)
}

/// What [`count_listed`] writes.
const COUNT_LISTED: &str =
    "INSERT INTO automations_run_counts (tenant_id, scenario_id, status, runs)
     VALUES (?1, ?2, ?3, ?4)
     ON CONFLICT (tenant_id, scenario_id, status) DO UPDATE SET runs = runs + excluded.runs";

/// Moves the count of a scenario's runs listed under `status` by `by`, in
/// the transaction of the write that lists them (or drops them).
pub(super) async fn count_listed(
    tx: &Connection,
    tenant_id: &str,
    scenario_id: &str,
    status: &str,
    by: i64,
) -> anyhow::Result<()> {
    execute(tx, COUNT_LISTED, (tenant_id, scenario_id, status, by)).await?;
    Ok(())
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

/// What [`run_known`] reads, by the table's key: the planner does not pick
/// it by itself, as the indexes of a scenario's runs lead with the tenant
/// too (and cover the id).
pub(super) const RUN_KNOWN: &str = "SELECT 1 FROM automations_runs
        INDEXED BY sqlite_autoindex_automations_runs_1
     WHERE tenant_id = ?1 AND id = ?2";

/// Whether the read model holds a run: it was started, by whichever node.
pub async fn run_known(db: &Db, tenant_id: &str, run_id: &str) -> anyhow::Result<bool> {
    let found: Option<(i64,)> = db.fetch_optional(RUN_KNOWN, (tenant_id, run_id)).await?;
    Ok(found.is_some())
}

/// Where the rows [`prune_runs`] drops at once stop: the last update of
/// the `?2 + 1`-th run, the longest ended first, of those that ended before
/// `?1`. Off the index of the ended runs, as the two statements after it.
pub(super) const PRUNE_UNTIL: &str = "SELECT updated_at FROM automations_runs
        INDEXED BY automations_runs_ended
     WHERE next_at IS NULL AND updated_at < ?1
     ORDER BY updated_at LIMIT 1 OFFSET ?2";

/// The runs [`prune_runs`] drops, as they are counted: by tenant, scenario
/// and status, with how many.
pub(super) const PRUNED_COUNTS: &str =
    "SELECT tenant_id, scenario_id, status, count(*) FROM automations_runs
        INDEXED BY automations_runs_ended
     WHERE next_at IS NULL AND updated_at < ?1
     GROUP BY tenant_id, scenario_id, status";

/// What [`prune_runs`] drops: every run that ended before `?1`.
pub(super) const PRUNE_RUNS: &str = "DELETE FROM automations_runs
        INDEXED BY automations_runs_ended
     WHERE next_at IS NULL AND updated_at < ?1";

/// Drops the rows of the runs that ended before `before`, and their counts
/// with them; how many. The store keeps the runs. About `most` go at a
/// time, the longest ended first (the `most`-th takes along every run that
/// ended in the same second); the next call takes the rest. So the
/// transaction stays short however many wait: it moves counts that the runs
/// ending meanwhile move too, and a write that meets another on the same
/// row is run again, whole.
pub async fn prune_runs(db: &Db, before: u64, most: u32) -> anyhow::Result<u64> {
    let env = (before as i64, i64::from(most.max(1)));
    db.write(&env, |tx, (before, most)| {
        Box::pin(async move {
            let last: Option<(i64,)> = fetch_optional(tx, PRUNE_UNTIL, (*before, most - 1)).await?;
            let until = last.map_or(*before, |(at,)| Ord::min(at + 1, *before));
            let counts: Vec<(String, String, String, i64)> =
                fetch_all(tx, PRUNED_COUNTS, (until,)).await?;
            for (tenant_id, scenario_id, status, runs) in &counts {
                count_listed(tx, tenant_id, scenario_id, status, -runs).await?;
            }
            execute(tx, PRUNE_RUNS, (until,)).await
        })
    })
    .await
}
