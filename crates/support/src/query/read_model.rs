//! The subscription that keeps `support_messages` current for one tenant.
//!
//! The handler reloads the strict write model of the message, opens it with
//! its key and rewrites its row, so delivery may repeat (the subscription is
//! at-least-once) without the row drifting; an erased message loses its row.
//! A key not readable yet (on its way from another node) fails the handler,
//! which is retried. It runs on the tenant's own executor (its cursor lives
//! there) and writes into the shared read-model database it is given.

use std::sync::Arc;

use evento::{
    Executor,
    metadata::RawEvent,
    subscription::{Context, SubscriptionBuilder},
};
use sqlx::SqlitePool;
use timada_core::Keys;

use crate::aggregator::Message as MessageStream;
use crate::command::{Message, message_projection};

/// The cursor identity of the read-model subscription, in each tenant's store.
pub const READ_MODEL_SUBSCRIPTION: &str = "timada-support/read-models";

/// The tenant whose stream a subscription follows, handed to its handlers.
#[derive(Debug, Clone)]
pub struct TenantId(pub String);

/// Where the tenant's keys are.
#[derive(Clone)]
struct TenantKeys(Arc<dyn Keys>);

/// Builds the subscription of one tenant over every routing key. `pool` is
/// the shared read-model database and must be allowed to write; `keys`
/// opens the messages. Start it with `.start(&tenant_executor)` and keep the
/// handle for shutdown.
pub fn read_model_subscription<E: Executor>(
    pool: SqlitePool,
    tenant_id: &str,
    keys: Arc<dyn Keys>,
) -> SubscriptionBuilder<E> {
    SubscriptionBuilder::new(READ_MODEL_SUBSCRIPTION)
        .data(pool)
        .data(TenantId(tenant_id.to_owned()))
        .data(TenantKeys(keys))
        .handler(on_message_event())
        .any_routing_key()
        .retry(u8::MAX)
}

#[evento::subscription_all]
async fn on_message_event<E: Executor>(
    context: &Context<'_, E>,
    event: RawEvent<MessageStream>,
) -> anyhow::Result<()> {
    let pool: SqlitePool = context.extract();
    let tenant: TenantId = context.extract();
    let keys: TenantKeys = context.extract();
    let Some(state) = message_projection()
        .load(&event.aggregate_id)
        .execute(context.executor)
        .await?
    else {
        return Ok(());
    };
    if state.forgotten {
        return delete_message(&pool, &tenant.0, &state.id).await;
    }
    let message = state
        .open_with(&*keys.0)
        .await?
        .ok_or_else(|| anyhow::anyhow!("the key of message {} is not readable yet", state.id))?;
    upsert_message(&pool, &tenant.0, &message).await
}

/// Drops the row of an erased message.
pub async fn delete_message(
    pool: &SqlitePool,
    tenant_id: &str,
    message_id: &str,
) -> anyhow::Result<()> {
    sqlx::query("DELETE FROM support_messages WHERE tenant_id = ?1 AND id = ?2")
        .bind(tenant_id)
        .bind(message_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Writes the row of a message as the write model sees it, opened.
pub async fn upsert_message(
    pool: &SqlitePool,
    tenant_id: &str,
    message: &Message,
) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO support_messages (tenant_id, id, first_name, last_name, email, body,
            language, status, received_at, handled_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
         ON CONFLICT (tenant_id, id) DO UPDATE SET
            status = excluded.status, handled_at = excluded.handled_at,
            updated_at = excluded.updated_at",
    )
    .bind(tenant_id)
    .bind(&message.id)
    .bind(&message.first_name)
    .bind(&message.last_name)
    .bind(&message.email)
    .bind(&message.body)
    .bind(&message.language)
    .bind(message.status.as_str())
    .bind(message.received_at as i64)
    .bind(message.handled_at.map(|at| at as i64))
    .bind(message.updated_at as i64)
    .execute(pool)
    .await?;
    Ok(())
}
