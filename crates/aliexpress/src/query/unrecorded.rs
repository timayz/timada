//! What AliExpress took for a forward and the store would not record: kept
//! on the node that placed it, in the read-model database, which takes
//! writes while the cluster refuses them, until a sweep records it. A
//! refusal can outlast what placing may wait for (a node rejoining, a
//! backlog at its cap); without this, the AliExpress order numbers would be
//! in the log alone, no capture asked, and the hold left to lapse while the
//! goods ship. One row per placement, so a second one of the same forward
//! keeps the first's numbers. The table survives a copy from a peer: no peer
//! holds it.

use timada_core::db::{Db, FromRow};

use crate::command::Placement;

/// One placement waiting to be recorded.
#[derive(Debug, Clone, FromRow, PartialEq, Eq)]
pub struct UnrecordedRow {
    pub tenant_id: String,
    pub forward_id: String,
    /// The AliExpress orders it took, space-separated: what tells it from
    /// another placement of the same forward.
    pub ae_order_ids: String,
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

/// The key a placement is kept under among the forward's.
fn key_of(placement: &Placement) -> String {
    placement.ae_order_ids.join(" ")
}

/// Keeps on this node what AliExpress took at `at` (unix seconds) for the
/// forward `forward_id` of the order `order_id`, whose record failed with
/// `error`. Kept again, the same placement keeps its first time; another
/// placement of the same forward is kept beside it.
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
        "INSERT INTO aliexpress_unrecorded (tenant_id, forward_id, ae_order_ids, order_id,
            placement, placed_at, tries, last_error)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0, ?7)
         ON CONFLICT (tenant_id, forward_id, ae_order_ids) DO UPDATE SET
            last_error = excluded.last_error",
        (
            tenant_id,
            forward_id,
            key_of(placement),
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
        "SELECT tenant_id, forward_id, ae_order_ids, order_id, placement, placed_at, tries,
            last_error
         FROM aliexpress_unrecorded
         ORDER BY placed_at, tenant_id, forward_id, ae_order_ids LIMIT ?1",
        (limit,),
    )
    .await
}

/// A try to record `row` failed again, with `error`.
pub async fn note_unrecorded_try(db: &Db, row: &UnrecordedRow, error: &str) -> anyhow::Result<()> {
    db.execute(
        "UPDATE aliexpress_unrecorded SET tries = tries + 1, last_error = ?4
         WHERE tenant_id = ?1 AND forward_id = ?2 AND ae_order_ids = ?3",
        (
            row.tenant_id.as_str(),
            row.forward_id.as_str(),
            row.ae_order_ids.as_str(),
            error,
        ),
    )
    .await?;
    Ok(())
}

/// `row` recorded at last: no longer kept.
pub async fn forget_unrecorded(db: &Db, row: &UnrecordedRow) -> anyhow::Result<()> {
    db.execute(
        "DELETE FROM aliexpress_unrecorded
         WHERE tenant_id = ?1 AND forward_id = ?2 AND ae_order_ids = ?3",
        (
            row.tenant_id.as_str(),
            row.forward_id.as_str(),
            row.ae_order_ids.as_str(),
        ),
    )
    .await?;
    Ok(())
}
