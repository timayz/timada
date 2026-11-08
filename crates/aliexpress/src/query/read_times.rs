//! When this node last read each AliExpress item and each open AliExpress
//! order, kept on the node (`aliexpress_read_times`) rather than in events.
//!
//! This is the one fact about an item that the store does not hold, on
//! purpose: a read that changes nothing records no event, and the sweep reads
//! every linked item of every shop once a period, for as long as the shop
//! lives. An event per read would spend a consensus round on each and run
//! an item's stream into the 65,535 versions a stream holds within a few
//! years, for a timestamp only this node's sweep goes by: the items and the
//! orders are shared out among the nodes up ([`Share`](crate::Share)), so
//! the node that reads one is the node that needs to know when it last did.
//! What the store does know (the last read recorded, the last one asked
//! for, the order's last change) is read beside it, and the later of the two
//! stands ([`LinkRow::last_synced_at`](super::LinkRow::last_synced_at)).
//!
//! The table lives in the read-model database, and survives a reset of the
//! read models (it is left out of [`TABLES`](super::TABLES)): a node copying
//! its store from a peer keeps knowing what it read, and no peer could tell
//! it. A node that lost it reads each of its items once more, a period
//! early, which is all the harm a lost row does. A row outlives its use
//! otherwise: the read-model subscription drops it ([`forget`]) once the
//! item is unlinked or the order is no longer read again.

use timada_core::db::Db;

/// A read of the link worked at `at` (unix seconds), whether or not it found
/// anything to record.
pub async fn note_read(db: &Db, tenant_id: &str, link_id: &str, at: u64) -> anyhow::Result<()> {
    db.execute(
        "INSERT INTO aliexpress_read_times (tenant_id, id, tried_at, synced_at)
         VALUES (?1, ?2, ?3, ?3)
         ON CONFLICT (tenant_id, id) DO UPDATE SET
            tried_at = MAX(COALESCE(tried_at, 0), excluded.tried_at),
            synced_at = MAX(COALESCE(synced_at, 0), excluded.synced_at)",
        (tenant_id, link_id, at as i64),
    )
    .await?;
    Ok(())
}

/// A read of the link was asked for or made at `at` (unix seconds), which is
/// what makes it due again a period later: one that failed, or a shop that
/// cannot read for now, takes no turn away from the others meanwhile.
pub async fn note_tried(db: &Db, tenant_id: &str, link_id: &str, at: u64) -> anyhow::Result<()> {
    db.execute(
        "INSERT INTO aliexpress_read_times (tenant_id, id, tried_at)
         VALUES (?1, ?2, ?3)
         ON CONFLICT (tenant_id, id) DO UPDATE SET
            tried_at = MAX(COALESCE(tried_at, 0), excluded.tried_at)",
        (tenant_id, link_id, at as i64),
    )
    .await?;
    Ok(())
}

/// The forward's orders were read at `at` (unix seconds), whether or not the
/// read worked or found anything: it waits for the next period.
pub async fn note_forward_read(
    db: &Db,
    tenant_id: &str,
    forward_id: &str,
    at: u64,
) -> anyhow::Result<()> {
    note_tried(db, tenant_id, forward_id, at).await
}

/// The node no longer needs to know when it last read `id` (an item that
/// is unlinked, an order no longer read again): its row goes. Nothing to
/// drop is fine, and a read noted later makes a row again.
pub async fn forget(db: &Db, tenant_id: &str, id: &str) -> anyhow::Result<()> {
    db.execute(
        "DELETE FROM aliexpress_read_times WHERE tenant_id = ?1 AND id = ?2",
        (tenant_id, id),
    )
    .await?;
    Ok(())
}
