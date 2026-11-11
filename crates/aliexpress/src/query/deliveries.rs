//! What delivering each imported item last cost and took, kept on the node
//! that asked (`aliexpress_deliveries`): a read of an item asks AliExpress
//! for its combinations in one call and for its delivery in another, and the
//! delivery seldom moves on its own. A routine read reuses the answer while
//! the combination it was asked for still costs the same and the answer is
//! not too old ([`crate::Delivery`]), so the sweep spends one call on most
//! items rather than two. A node that reads an item for the first time asks
//! once more; what it finds is this node's to keep, like its read times.

use timada_catalog::DeliveryDays;
use timada_core::db::Db;

use crate::model::Delivery;

/// A delivery as read at `read_at` (unix seconds).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeliveryRead {
    pub delivery: Delivery,
    pub read_at: u64,
}

/// Keeps what delivering the link's item costs, as read at `at`.
pub async fn note_delivery(
    db: &Db,
    tenant_id: &str,
    link_id: &str,
    delivery: &Delivery,
    at: u64,
) -> anyhow::Result<()> {
    db.execute(
        "INSERT INTO aliexpress_deliveries (tenant_id, id, sku_id, sku_cost_cents, fee_cents,
            min_days, max_days, read_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT (tenant_id, id) DO UPDATE SET
            sku_id = excluded.sku_id, sku_cost_cents = excluded.sku_cost_cents,
            fee_cents = excluded.fee_cents, min_days = excluded.min_days,
            max_days = excluded.max_days, read_at = excluded.read_at",
        (
            tenant_id,
            link_id,
            delivery.sku_id.as_str(),
            delivery.sku_cost_cents as i64,
            delivery.fee_cents as i64,
            delivery.days.map(|days| i64::from(days.min)),
            delivery.days.map(|days| i64::from(days.max)),
            at as i64,
        ),
    )
    .await?;
    Ok(())
}

/// What delivering the link's item last cost, as this node read it; `None`
/// before it did.
pub async fn delivery_read(
    db: &Db,
    tenant_id: &str,
    link_id: &str,
) -> anyhow::Result<Option<DeliveryRead>> {
    let row: Option<DeliveryRow> = db
        .fetch_optional(
            "SELECT sku_id, sku_cost_cents, fee_cents, min_days, max_days, read_at
             FROM aliexpress_deliveries WHERE tenant_id = ?1 AND id = ?2",
            (tenant_id, link_id),
        )
        .await?;
    Ok(row.map(
        |(sku_id, sku_cost_cents, fee_cents, min_days, max_days, read_at)| DeliveryRead {
            delivery: Delivery {
                sku_id,
                sku_cost_cents: sku_cost_cents.max(0) as u64,
                fee_cents: fee_cents.max(0) as u64,
                days: match (min_days, max_days) {
                    (Some(min), Some(max)) => DeliveryDays::new(
                        min.clamp(0, i64::from(u16::MAX)) as u16,
                        max.clamp(0, i64::from(u16::MAX)) as u16,
                    )
                    .ok(),
                    _ => None,
                },
            },
            read_at: read_at.max(0) as u64,
        },
    ))
}

/// A row of `aliexpress_deliveries`: the combination asked about, its cost,
/// the fee, the days at the soonest and the latest, when it was read.
type DeliveryRow = (String, i64, i64, Option<i64>, Option<i64>, i64);

/// The item is no longer read: what delivering it cost goes.
pub async fn forget_delivery(db: &Db, tenant_id: &str, link_id: &str) -> anyhow::Result<()> {
    db.execute(
        "DELETE FROM aliexpress_deliveries WHERE tenant_id = ?1 AND id = ?2",
        (tenant_id, link_id),
    )
    .await?;
    Ok(())
}
