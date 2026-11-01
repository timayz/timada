//! `automations_checkouts`: the carts asked to be paid for, as the
//! subscription that starts runs sees them (it follows the cart events, so
//! it knows when an order follows), and what the runner looks over for
//! checkouts left unpaid.

use timada_core::db::Db;

use crate::catalog::CHECKOUT_ABANDONED;

/// Records a cart's latest checkout request; a new one starts over.
pub async fn record_checkout(
    db: &Db,
    tenant_id: &str,
    cart_id: &str,
    request_id: &str,
    requested_at: u64,
) -> anyhow::Result<()> {
    db.execute(
        "INSERT INTO automations_checkouts (tenant_id, cart_id, request_id, requested_at,
            ordered, detected)
         VALUES (?1, ?2, ?3, ?4, 0, 0)
         ON CONFLICT (tenant_id, cart_id) DO UPDATE SET
            request_id = excluded.request_id, requested_at = excluded.requested_at,
            detected = 0
         WHERE automations_checkouts.ordered = 0",
        (tenant_id, cart_id, request_id, requested_at as i64),
    )
    .await?;
    Ok(())
}

/// Records that the cart became an order: it is no longer left.
pub async fn record_checked_out(db: &Db, tenant_id: &str, cart_id: &str) -> anyhow::Result<()> {
    db.execute(
        "UPDATE automations_checkouts SET ordered = 1 WHERE tenant_id = ?1 AND cart_id = ?2",
        (tenant_id, cart_id),
    )
    .await?;
    Ok(())
}

/// Records that a request was found left.
pub async fn record_detected(
    db: &Db,
    tenant_id: &str,
    cart_id: &str,
    request_id: &str,
) -> anyhow::Result<()> {
    db.execute(
        "UPDATE automations_checkouts SET detected = 1
         WHERE tenant_id = ?1 AND cart_id = ?2 AND request_id = ?3",
        (tenant_id, cart_id, request_id),
    )
    .await?;
    Ok(())
}

/// The checkouts asked for between `from` and `to` (unix seconds) that no
/// order followed and nobody found left yet, in shops with a scenario on
/// that trigger: `(tenant id, cart id, request id)`.
pub async fn left_checkouts(
    db: &Db,
    from: u64,
    to: u64,
    limit: u32,
) -> anyhow::Result<Vec<(String, String, String)>> {
    db.fetch_all(
        "SELECT c.tenant_id, c.cart_id, c.request_id FROM automations_checkouts c
         WHERE c.ordered = 0 AND c.detected = 0
           AND c.requested_at >= ?1 AND c.requested_at <= ?2
           AND EXISTS (SELECT 1 FROM automations_listeners l
                       WHERE l.tenant_id = c.tenant_id AND l.trigger = ?3
                         AND l.state = 'active')
         ORDER BY c.requested_at
         LIMIT ?4",
        (from as i64, to as i64, CHECKOUT_ABANDONED, i64::from(limit)),
    )
    .await
}

/// Drops the checkouts asked for before `before`: too old to be found
/// left; how many.
pub async fn prune_checkouts(db: &Db, before: u64) -> anyhow::Result<u64> {
    let done = db
        .execute(
            "DELETE FROM automations_checkouts WHERE requested_at < ?1",
            (before as i64,),
        )
        .await?;
    Ok(done)
}
