//! `automations_starts`: the runs this node saw an event start that fell to
//! another node to start. That node starts each as it sees the event; this
//! one keeps it here until the host's runner looks again a while later and
//! starts it if that node did not (it stopped, or the nodes disagreed for a
//! moment on who is up). The while counts from when this node kept it, not
//! from the event: a node going through events late (a burst, a restart)
//! is about as late as the node they fall to. A node's own, kept by the
//! subscription that starts runs, like `automations_listeners`.

use timada_core::db::{Db, FromRow};

use crate::command::StartRun;

/// One row of `automations_starts`.
#[derive(Debug, Clone, FromRow, PartialEq, Eq)]
pub struct OwedStart {
    pub tenant_id: String,
    pub run_id: String,
    pub scenario_id: String,
    pub scenario_version: i64,
    pub trigger: String,
    pub trigger_event_id: String,
    pub subject_id: String,
    /// Unix seconds when this node kept it.
    pub kept_at: i64,
}

impl OwedStart {
    /// What starts the run.
    pub fn start(&self) -> StartRun {
        StartRun {
            scenario_id: self.scenario_id.clone(),
            scenario_version: self.scenario_version as u32,
            trigger: self.trigger.clone(),
            trigger_event_id: self.trigger_event_id.clone(),
            subject_id: self.subject_id.clone(),
        }
    }
}

/// Keeps a run another node is to start, at `kept_at` (unix seconds). Seen
/// again, it stays as it was.
pub async fn owe_start(
    db: &Db,
    tenant_id: &str,
    run_id: &str,
    start: &StartRun,
    kept_at: u64,
) -> anyhow::Result<()> {
    db.execute(
        "INSERT INTO automations_starts (tenant_id, run_id, scenario_id, scenario_version,
            trigger, trigger_event_id, subject_id, kept_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT (tenant_id, run_id) DO NOTHING",
        (
            tenant_id,
            run_id,
            start.scenario_id.as_str(),
            i64::from(start.scenario_version),
            start.trigger.as_str(),
            start.trigger_event_id.as_str(),
            start.subject_id.as_str(),
            kept_at as i64,
        ),
    )
    .await?;
    Ok(())
}

/// What [`starts_owed`] reads: off the index of when they were kept, in
/// its order.
pub(super) const STARTS_OWED: &str =
    "SELECT * FROM automations_starts INDEXED BY automations_starts_kept
     WHERE kept_at <= ?1
     ORDER BY kept_at
     LIMIT ?2";

/// The runs kept for other nodes at `before` or earlier, the oldest first.
pub async fn starts_owed(db: &Db, before: u64, limit: u32) -> anyhow::Result<Vec<OwedStart>> {
    db.fetch_all(STARTS_OWED, (before as i64, i64::from(limit)))
        .await
}

/// Forgets a run kept for another node: started, by it or by this one.
pub async fn settle_start(db: &Db, tenant_id: &str, run_id: &str) -> anyhow::Result<()> {
    db.execute(
        "DELETE FROM automations_starts WHERE tenant_id = ?1 AND run_id = ?2",
        (tenant_id, run_id),
    )
    .await?;
    Ok(())
}

/// Forgets the runs kept for other nodes before `before`, which this node
/// did not look at since (it was away): too late to start any more; how
/// many.
pub async fn prune_starts(db: &Db, before: u64) -> anyhow::Result<u64> {
    db.execute(
        "DELETE FROM automations_starts WHERE kept_at < ?1",
        (before as i64,),
    )
    .await
}
