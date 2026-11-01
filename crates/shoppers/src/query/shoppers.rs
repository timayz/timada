//! Queries over `shoppers_shoppers`, for the customers page of a tenant.

use timada_core::db::{Db, FromRow};

/// One shopper of a tenant, with the orders placed with their address.
#[derive(Debug, Clone, FromRow, PartialEq, Eq)]
pub struct ShopperRow {
    pub id: String,
    /// Normalized.
    pub email: String,
    /// Unix seconds of the first sign-in.
    pub registered_at: i64,
    pub address_count: i64,
    /// Every order placed with the address, guest ones included, read from
    /// the orders context's `orders_orders` (same database).
    pub order_count: i64,
}

/// The tenant's shoppers, newest first.
pub async fn shoppers_of_tenant(
    db: &Db,
    tenant_id: &str,
    limit: u32,
    offset: u32,
) -> anyhow::Result<Vec<ShopperRow>> {
    db.fetch_all(
        "SELECT s.id, s.email, s.registered_at, s.address_count,
            (SELECT COUNT(*) FROM orders_orders o
             WHERE o.tenant_id = s.tenant_id AND o.email = s.email) AS order_count
         FROM shoppers_shoppers s
         WHERE s.tenant_id = ?1
         ORDER BY s.registered_at DESC, s.id DESC
         LIMIT ?2 OFFSET ?3",
        (tenant_id, i64::from(limit), i64::from(offset)),
    )
    .await
}

/// How many shoppers the tenant has.
pub async fn count_shoppers(db: &Db, tenant_id: &str) -> anyhow::Result<u64> {
    let (count,): (i64,) = db
        .fetch_one(
            "SELECT COUNT(*) FROM shoppers_shoppers WHERE tenant_id = ?1",
            (tenant_id,),
        )
        .await?;
    Ok(count as u64)
}
