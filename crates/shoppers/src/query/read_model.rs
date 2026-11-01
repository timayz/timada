//! The subscription that keeps `shoppers_shoppers` current for every tenant.
//!
//! The handler reloads the strict write model of the shopper and rewrites its
//! row, so delivery may repeat (the subscription is at-least-once) without the
//! numbers drifting; a shopper who deleted their account loses their row. One
//! subscription follows every tenant's streams (its cursor lives in the
//! store): each row carries the tenant of the event that made it, and it
//! writes into the read-model database it is given.
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

use timada_core::db::Db;

use crate::command::{Shopper, shopper_projection};
use timada_core::keys::KeyRing as ShopperKeyRing;

/// The cursor identity of the read-model subscription, in the store.
pub const READ_MODEL_SUBSCRIPTION: &str = "timada-shoppers/read-models";

/// The shopper keys of every tenant, handed to the handlers.
#[derive(Clone)]
struct Ring(Arc<dyn ShopperKeyRing>);

/// Builds the subscription over every tenant and routing key. `db` is the
/// read-model database and must be allowed to write; `keys` holds each
/// tenant's shopper keys. Start it with `.start(&store)` and keep the handle
/// for shutdown.
pub fn read_model_subscription<E: Executor>(
    db: Db,
    keys: Arc<dyn ShopperKeyRing>,
) -> SubscriptionBuilder<E> {
    SubscriptionBuilder::new(READ_MODEL_SUBSCRIPTION)
        .data(db)
        .data(Ring(keys))
        .handler(on_shopper_event())
        .any_routing_key()
        .retry(u8::MAX)
}

#[evento::subscription_all]
async fn on_shopper_event<E: Executor>(
    context: &Context<'_, E>,
    event: RawEvent<crate::aggregator::Shopper>,
) -> anyhow::Result<()> {
    let db: Db = context.extract();
    let keys: Ring = context.extract();
    let Some(state) = shopper_projection()
        .load(&event.aggregate_id)
        .tenant(&event.tenant)
        .execute(context.executor)
        .await?
    else {
        return Ok(());
    };
    if state.forgotten {
        return delete_shopper(&db, &event.tenant, &state.id).await;
    }
    // Not forgotten, so the key must exist: until it can be read, retry.
    let keys = keys.0.for_tenant(&event.tenant);
    let shopper = state
        .open_with(&*keys)
        .await?
        .ok_or_else(|| anyhow::anyhow!("the key of shopper {} is missing", state.id))?;
    upsert_shopper(&db, &event.tenant, &shopper, event.timestamp).await
}

/// Drops the row of a shopper who deleted their account.
pub async fn delete_shopper(db: &Db, tenant_id: &str, shopper_id: &str) -> anyhow::Result<()> {
    db.execute(
        "DELETE FROM shoppers_shoppers WHERE tenant_id = ?1 AND id = ?2",
        (tenant_id, shopper_id),
    )
    .await?;
    Ok(())
}

/// Writes the row of a shopper as the write model sees it; `updated_at` is
/// unix seconds.
pub async fn upsert_shopper(
    db: &Db,
    tenant_id: &str,
    shopper: &Shopper,
    updated_at: u64,
) -> anyhow::Result<()> {
    db.execute(
        "INSERT INTO shoppers_shoppers (tenant_id, id, email, registered_at, address_count,
            updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT (tenant_id, id) DO UPDATE SET
            email = excluded.email,
            registered_at = excluded.registered_at,
            address_count = excluded.address_count,
            updated_at = MAX(updated_at, excluded.updated_at)",
        (
            tenant_id,
            shopper.id.as_str(),
            shopper.email.as_str(),
            shopper.registered_at as i64,
            shopper.addresses.len() as i64,
            updated_at as i64,
        ),
    )
    .await?;
    Ok(())
}
