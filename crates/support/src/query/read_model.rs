//! The subscription that keeps `support_messages` current for every tenant.
//!
//! The handler reloads the strict write model of the message, opens it with
//! its key and rewrites its row, so delivery may repeat (the subscription is
//! at-least-once) without the row drifting; an erased message loses its row.
//! A key not readable yet (on its way from another node) fails the handler,
//! which is retried. One subscription follows every tenant's streams (its
//! cursor lives in the store): each row carries the tenant of the event that
//! made it, and the rows live in the store's own database ([`Db`]).

use std::sync::Arc;

use evento::{
    Executor,
    metadata::RawEvent,
    subscription::{Context, SubscriptionBuilder},
};
use timada_core::db::Db;
use timada_core::keys::KeyRing;

use crate::aggregator::Message as MessageStream;
use crate::command::{Message, message_projection};

/// The cursor identity of the read-model subscription, in the store.
pub const READ_MODEL_SUBSCRIPTION: &str = "timada-support/read-models";

/// Where every tenant's keys are.
#[derive(Clone)]
struct Ring(Arc<dyn KeyRing>);

/// Builds the subscription over every tenant and routing key. `keys` holds
/// each tenant's keys, which open the messages. Start it with
/// `.start(&store)` and keep the handle for shutdown.
pub fn read_model_subscription<E: Executor>(
    db: Db,
    keys: Arc<dyn KeyRing>,
) -> SubscriptionBuilder<E> {
    SubscriptionBuilder::new(READ_MODEL_SUBSCRIPTION)
        .data(db)
        .data(Ring(keys))
        .handler(on_message_event())
        .any_routing_key()
        .retry(u8::MAX)
}

#[evento::subscription_all]
async fn on_message_event<E: Executor>(
    context: &Context<'_, E>,
    event: RawEvent<MessageStream>,
) -> anyhow::Result<()> {
    let db: Db = context.extract();
    let keys: Ring = context.extract();
    let Some(state) = message_projection()
        .load(&event.aggregate_id)
        .tenant(&event.tenant)
        .execute(context.executor)
        .await?
    else {
        return Ok(());
    };
    if state.forgotten {
        return delete_message(&db, &event.tenant, &state.id).await;
    }
    let keys = keys.0.for_tenant(&event.tenant);
    let message = state
        .open_with(&*keys)
        .await?
        .ok_or_else(|| anyhow::anyhow!("the key of message {} is not readable yet", state.id))?;
    upsert_message(&db, &event.tenant, &message).await
}

/// Drops the row of an erased message.
pub async fn delete_message(db: &Db, tenant_id: &str, message_id: &str) -> anyhow::Result<()> {
    db.execute(
        "DELETE FROM support_messages WHERE tenant_id = ?1 AND id = ?2",
        (tenant_id, message_id),
    )
    .await?;
    Ok(())
}

/// Writes the row of a message as the write model sees it, opened.
pub async fn upsert_message(db: &Db, tenant_id: &str, message: &Message) -> anyhow::Result<()> {
    db.execute(
        "INSERT INTO support_messages (tenant_id, id, first_name, last_name, email, body,
            language, status, received_at, handled_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
         ON CONFLICT (tenant_id, id) DO UPDATE SET
            status = excluded.status, handled_at = excluded.handled_at,
            updated_at = excluded.updated_at",
        (
            tenant_id,
            message.id.as_str(),
            message.first_name.as_str(),
            message.last_name.as_str(),
            message.email.as_str(),
            message.body.as_str(),
            message.language.as_deref(),
            message.status.as_str(),
            message.received_at as i64,
            message.handled_at.map(|at| at as i64),
            message.updated_at as i64,
        ),
    )
    .await?;
    Ok(())
}
