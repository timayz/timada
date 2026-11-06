//! What AliExpress took for a forward and the store would not record: kept
//! on the node that placed it, in the read-model database, which takes
//! writes while the cluster refuses them, until a sweep records it. A
//! refusal can outlast what placing may wait for (a node rejoining, a
//! backlog at its cap); without this, the AliExpress order numbers would be
//! in the log alone, no capture asked, and the hold left to lapse while the
//! goods ship. The table survives a copy from a peer: no peer holds it.

use timada_core::db::{Db, FromRow};

use crate::command::Placement;

/// One placement waiting to be recorded.
#[derive(Debug, Clone, FromRow, PartialEq, Eq)]
pub struct UnrecordedRow {
    pub tenant_id: String,
    pub forward_id: String,
    pub order_id: String,
    /// The [`Placement`], as JSON.
    pub placement: String,
    /// Unix seconds AliExpress took it.
    pub placed_at: i64,
    /// How many sweeps tried to record it.
    pub tries: i64,
    pub last_error: Option<String>,
}

impl UnrecordedRow {
    /// What AliExpress took.
    pub fn placement(&self) -> anyhow::Result<Placement> {
        Ok(serde_json::from_str(&self.placement)?)
    }
}

/// Keeps on this node what AliExpress took at `at` (unix seconds) for the
/// forward `forward_id` of the order `order_id`, whose record failed with
/// `error`. Kept again, the same forward keeps its first time.
pub async fn keep_unrecorded(
    db: &Db,
    tenant_id: &str,
    forward_id: &str,
    order_id: &str,
    placement: &Placement,
    at: u64,
    error: &str,
) -> anyhow::Result<()> {
    db.execute(
        "INSERT INTO aliexpress_unrecorded (tenant_id, forward_id, order_id, placement,
            placed_at, tries, last_error)
         VALUES (?1, ?2, ?3, ?4, ?5, 0, ?6)
         ON CONFLICT (tenant_id, forward_id) DO UPDATE SET
            placement = excluded.placement, last_error = excluded.last_error",
        (
            tenant_id,
            forward_id,
            order_id,
            serde_json::to_string(placement)?,
            at as i64,
            error,
        ),
    )
    .await?;
    Ok(())
}

/// The placements this node keeps, the oldest first.
pub async fn unrecorded(db: &Db, limit: i64) -> anyhow::Result<Vec<UnrecordedRow>> {
    db.fetch_all(
        "SELECT tenant_id, forward_id, order_id, placement, placed_at, tries, last_error
         FROM aliexpress_unrecorded ORDER BY placed_at, tenant_id, forward_id LIMIT ?1",
        (limit,),
    )
    .await
}

/// A try to record it failed again, with `error`.
pub async fn note_unrecorded_try(
    db: &Db,
    tenant_id: &str,
    forward_id: &str,
    error: &str,
) -> anyhow::Result<()> {
    db.execute(
        "UPDATE aliexpress_unrecorded SET tries = tries + 1, last_error = ?3
         WHERE tenant_id = ?1 AND forward_id = ?2",
        (tenant_id, forward_id, error),
    )
    .await?;
    Ok(())
}

/// Recorded at last: no longer kept.
pub async fn forget_unrecorded(db: &Db, tenant_id: &str, forward_id: &str) -> anyhow::Result<()> {
    db.execute(
        "DELETE FROM aliexpress_unrecorded WHERE tenant_id = ?1 AND forward_id = ?2",
        (tenant_id, forward_id),
    )
    .await?;
    Ok(())
}
