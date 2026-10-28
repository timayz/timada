//! `automations_checkouts`: the carts asked to be paid for, as the
//! subscription that starts runs sees them (it follows the cart events, so
//! it knows when an order follows), and what the runner looks over for
//! checkouts left unpaid.

use sqlx::SqlitePool;

use crate::catalog::CHECKOUT_ABANDONED;

/// Records a cart's latest checkout request; a new one starts over.
pub async fn record_checkout(
    pool: &SqlitePool,
    tenant_id: &str,
    cart_id: &str,
    request_id: &str,
    requested_at: u64,
) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO automations_checkouts (tenant_id, cart_id, request_id, requested_at,
            ordered, detected)
         VALUES (?1, ?2, ?3, ?4, 0, 0)
         ON CONFLICT (tenant_id, cart_id) DO UPDATE SET
            request_id = excluded.request_id, requested_at = excluded.requested_at,
            detected = 0
         WHERE automations_checkouts.ordered = 0",
    )
    .bind(tenant_id)
    .bind(cart_id)
    .bind(request_id)
    .bind(requested_at as i64)
    .execute(pool)
    .await?;
    Ok(())
}

/// Records that the cart became an order: it is no longer left.
pub async fn record_checked_out(
    pool: &SqlitePool,
    tenant_id: &str,
    cart_id: &str,
) -> anyhow::Result<()> {
    sqlx::query(
        "UPDATE automations_checkouts SET ordered = 1 WHERE tenant_id = ?1 AND cart_id = ?2",
    )
    .bind(tenant_id)
    .bind(cart_id)
    .execute(pool)
    .await?;
    Ok(())
}

/// Records that a request was found left.
pub async fn record_detected(
    pool: &SqlitePool,
    tenant_id: &str,
    cart_id: &str,
    request_id: &str,
) -> anyhow::Result<()> {
    sqlx::query(
        "UPDATE automations_checkouts SET detected = 1
         WHERE tenant_id = ?1 AND cart_id = ?2 AND request_id = ?3",
    )
    .bind(tenant_id)
    .bind(cart_id)
    .bind(request_id)
    .execute(pool)
    .await?;
    Ok(())
}

/// The checkouts asked for between `from` and `to` (unix seconds) that no
/// order followed and nobody found left yet, in shops with a scenario on
/// that trigger: `(tenant id, cart id, request id)`.
pub async fn left_checkouts(
    pool: &SqlitePool,
    from: u64,
    to: u64,
    limit: u32,
) -> anyhow::Result<Vec<(String, String, String)>> {
    Ok(sqlx::query_as(
        "SELECT c.tenant_id, c.cart_id, c.request_id FROM automations_checkouts c
         WHERE c.ordered = 0 AND c.detected = 0
           AND c.requested_at >= ?1 AND c.requested_at <= ?2
           AND EXISTS (SELECT 1 FROM automations_listeners l
                       WHERE l.tenant_id = c.tenant_id AND l.trigger = ?3
                         AND l.state = 'active')
         ORDER BY c.requested_at
         LIMIT ?4",
    )
    .bind(from as i64)
    .bind(to as i64)
    .bind(CHECKOUT_ABANDONED)
    .bind(i64::from(limit))
    .fetch_all(pool)
    .await?)
}

/// Drops the checkouts asked for before `before`: too old to be found
/// left; how many.
pub async fn prune_checkouts(pool: &SqlitePool, before: u64) -> anyhow::Result<u64> {
    let done = sqlx::query("DELETE FROM automations_checkouts WHERE requested_at < ?1")
        .bind(before as i64)
        .execute(pool)
        .await?;
    Ok(done.rows_affected())
}
