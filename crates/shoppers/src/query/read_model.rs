//! The subscription that keeps `shoppers_shoppers` current for one tenant.
//!
//! The handler reloads the strict write model of the shopper and rewrites its
//! row, so delivery may repeat (the subscription is at-least-once) without the
//! numbers drifting; a shopper who deleted their account loses their row. It
//! runs on the tenant's own executor (its cursor lives there) and writes into
//! the shared read-model database it is given.
//!
//! The row lists the address the shopper signs in with, so the handler opens
//! it with their key. A key this node cannot read yet (still on its way from
//! another node) fails the handler, which is retried until it arrives.

use evento::{
    Executor,
    metadata::RawEvent,
    subscription::{Context, SubscriptionBuilder},
};
use std::sync::Arc;

use sqlx::SqlitePool;

use crate::command::{Shopper, shopper_projection};
use crate::key::ShopperKeys;

/// The cursor identity of the read-model subscription, in each tenant's store.
pub const READ_MODEL_SUBSCRIPTION: &str = "timada-shoppers/read-models";

/// The tenant whose stream a subscription follows, handed to its handlers.
#[derive(Debug, Clone)]
pub struct TenantId(pub String);

/// The keys of one tenant's shoppers, handed to the handlers.
#[derive(Clone)]
struct TenantKeys(Arc<dyn ShopperKeys>);

/// Builds the subscription of one tenant over every routing key. `pool` is
/// the shared read-model database and must be allowed to write; `keys` are
/// the tenant's shopper keys. Start it with `.start(&tenant_executor)` and
/// keep the handle for shutdown.
pub fn read_model_subscription<E: Executor>(
    pool: SqlitePool,
    tenant_id: &str,
    keys: Arc<dyn ShopperKeys>,
) -> SubscriptionBuilder<E> {
    SubscriptionBuilder::new(READ_MODEL_SUBSCRIPTION)
        .data(pool)
        .data(TenantId(tenant_id.to_owned()))
        .data(TenantKeys(keys))
        .handler(on_shopper_event())
        .any_routing_key()
        .retry(u8::MAX)
}

#[evento::subscription_all]
async fn on_shopper_event<E: Executor>(
    context: &Context<'_, E>,
    event: RawEvent<crate::aggregator::Shopper>,
) -> anyhow::Result<()> {
    let pool: SqlitePool = context.extract();
    let tenant: TenantId = context.extract();
    let keys: TenantKeys = context.extract();
    let Some(state) = shopper_projection()
        .load(&event.aggregate_id)
        .execute(context.executor)
        .await?
    else {
        return Ok(());
    };
    if state.forgotten {
        return delete_shopper(&pool, &tenant.0, &state.id).await;
    }
    // Not forgotten, so the key must exist: until it can be read, retry.
    let shopper = state
        .open_with(&*keys.0)
        .await?
        .ok_or_else(|| anyhow::anyhow!("the key of shopper {} is missing", state.id))?;
    upsert_shopper(&pool, &tenant.0, &shopper, event.timestamp).await
}

/// Drops the row of a shopper who deleted their account.
pub async fn delete_shopper(
    pool: &SqlitePool,
    tenant_id: &str,
    shopper_id: &str,
) -> anyhow::Result<()> {
    sqlx::query("DELETE FROM shoppers_shoppers WHERE tenant_id = ?1 AND id = ?2")
        .bind(tenant_id)
        .bind(shopper_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Writes the row of a shopper as the write model sees it; `updated_at` is
/// unix seconds.
pub async fn upsert_shopper(
    pool: &SqlitePool,
    tenant_id: &str,
    shopper: &Shopper,
    updated_at: u64,
) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO shoppers_shoppers (tenant_id, id, email, registered_at, address_count,
            updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT (tenant_id, id) DO UPDATE SET
            email = excluded.email,
            registered_at = excluded.registered_at,
            address_count = excluded.address_count,
            updated_at = MAX(updated_at, excluded.updated_at)",
    )
    .bind(tenant_id)
    .bind(&shopper.id)
    .bind(&shopper.email)
    .bind(shopper.registered_at as i64)
    .bind(shopper.addresses.len() as i64)
    .bind(updated_at as i64)
    .execute(pool)
    .await?;
    Ok(())
}
