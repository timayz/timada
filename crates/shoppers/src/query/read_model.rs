//! The subscription that keeps `shoppers_shoppers` current for one tenant.
//!
//! The handler reloads the strict write model of the shopper and rewrites its
//! row, so delivery may repeat (the subscription is at-least-once) without the
//! numbers drifting. It runs on the tenant's own executor (its cursor lives
//! there) and writes into the shared read-model database it is given.

use evento::{
    Executor,
    metadata::RawEvent,
    subscription::{Context, SubscriptionBuilder},
};
use sqlx::SqlitePool;

use crate::aggregator::Shopper;
use crate::command::{ShopperState, shopper_projection};

/// The cursor identity of the read-model subscription, in each tenant's store.
pub const READ_MODEL_SUBSCRIPTION: &str = "timada-shoppers/read-models";

/// The tenant whose stream a subscription follows, handed to its handlers.
#[derive(Debug, Clone)]
pub struct TenantId(pub String);

/// Builds the subscription of one tenant over every routing key. `pool` is
/// the shared read-model database and must be allowed to write; start it
/// with `.start(&tenant_executor)` and keep the handle for shutdown.
pub fn read_model_subscription<E: Executor>(
    pool: SqlitePool,
    tenant_id: &str,
) -> SubscriptionBuilder<E> {
    SubscriptionBuilder::new(READ_MODEL_SUBSCRIPTION)
        .data(pool)
        .data(TenantId(tenant_id.to_owned()))
        .handler(on_shopper_event())
        .any_routing_key()
}

#[evento::subscription_all]
async fn on_shopper_event<E: Executor>(
    context: &Context<'_, E>,
    event: RawEvent<Shopper>,
) -> anyhow::Result<()> {
    let pool: SqlitePool = context.extract();
    let tenant: TenantId = context.extract();
    let Some(shopper) = shopper_projection()
        .load(&event.aggregate_id)
        .execute(context.executor)
        .await?
    else {
        return Ok(());
    };
    upsert_shopper(&pool, &tenant.0, &shopper, event.timestamp).await
}

/// Writes the row of a shopper as the write model sees it; `updated_at` is
/// unix seconds.
pub async fn upsert_shopper(
    pool: &SqlitePool,
    tenant_id: &str,
    shopper: &ShopperState,
    updated_at: u64,
) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO shoppers_shoppers (tenant_id, id, email, registered_at, address_count,
            updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT (tenant_id, id) DO UPDATE SET
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
