//! The subscription that keeps `support_messages` current for one tenant.
//!
//! The handler reloads the strict write model of the message and rewrites
//! its row, so delivery may repeat (the subscription is at-least-once)
//! without the row drifting. It runs on the tenant's own executor (its
//! cursor lives there) and writes into the shared read-model database it is
//! given.

use evento::{
    Executor,
    metadata::RawEvent,
    subscription::{Context, SubscriptionBuilder},
};
use sqlx::SqlitePool;

use crate::aggregator::Message;
use crate::command::{MessageState, message_projection};

/// The cursor identity of the read-model subscription, in each tenant's store.
pub const READ_MODEL_SUBSCRIPTION: &str = "timada-support/read-models";

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
        .handler(on_message_event())
        .any_routing_key()
}

#[evento::subscription_all]
async fn on_message_event<E: Executor>(
    context: &Context<'_, E>,
    event: RawEvent<Message>,
) -> anyhow::Result<()> {
    let pool: SqlitePool = context.extract();
    let tenant: TenantId = context.extract();
    let Some(message) = message_projection()
        .load(&event.aggregate_id)
        .execute(context.executor)
        .await?
    else {
        return Ok(());
    };
    upsert_message(&pool, &tenant.0, &message).await
}

/// Writes the row of a message as the write model sees it.
pub async fn upsert_message(
    pool: &SqlitePool,
    tenant_id: &str,
    message: &MessageState,
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
