//! The subscription that keeps `newsletter_subscribers`,
//! `newsletter_campaigns` and `newsletter_deliveries` current for every
//! tenant.
//!
//! Each handler reloads the strict write model of the stream and rewrites
//! its row (a subscriber's opened with their key), so delivery may repeat (the
//! subscription is at-least-once) without the row drifting; an erased
//! subscriber loses their row. A key not readable yet (on its way from
//! another node) fails the handler, which is retried. One subscription
//! follows every tenant's streams (its cursor lives in the store): each row
//! carries the tenant of the event that made it, and it writes into the
//! read-model database it is given.

use std::sync::Arc;

use evento::{
    Executor,
    metadata::RawEvent,
    subscription::{Context, SubscriptionBuilder},
};
use sqlx::SqlitePool;
use timada_core::keys::KeyRing;

use crate::aggregator::{
    Campaign as CampaignStream, Delivery as DeliveryStream, Subscriber as SubscriberStream,
};
use crate::command::{
    CampaignState, Subscriber, campaign_projection, delivery_projection, subscriber_projection,
};

/// The cursor identity of the read-model subscription, in the store.
pub const READ_MODEL_SUBSCRIPTION: &str = "timada-newsletter/read-models";

/// Where every tenant's keys are.
#[derive(Clone)]
struct Ring(Arc<dyn KeyRing>);

/// Builds the subscription over every tenant and routing key. `pool` is the
/// read-model database and must be allowed to write; `keys` holds each
/// tenant's keys, which open the addresses. Start it with `.start(&store)`
/// and keep the handle for shutdown.
pub fn read_model_subscription<E: Executor>(
    pool: SqlitePool,
    keys: Arc<dyn KeyRing>,
) -> SubscriptionBuilder<E> {
    SubscriptionBuilder::new(READ_MODEL_SUBSCRIPTION)
        .data(pool)
        .data(Ring(keys))
        .handler(on_subscriber_event())
        .handler(on_campaign_event())
        .handler(on_delivery_event())
        .any_routing_key()
        .retry(u8::MAX)
}

#[evento::subscription_all]
async fn on_subscriber_event<E: Executor>(
    context: &Context<'_, E>,
    event: RawEvent<SubscriberStream>,
) -> anyhow::Result<()> {
    let pool: SqlitePool = context.extract();
    let keys: Ring = context.extract();
    let Some(state) = subscriber_projection()
        .load(&event.aggregate_id)
        .tenant(&event.tenant)
        .execute(context.executor)
        .await?
    else {
        return Ok(());
    };
    if state.forgotten {
        return delete_subscriber(&pool, &event.tenant, &state.id).await;
    }
    let keys = keys.0.for_tenant(&event.tenant);
    let subscriber = state
        .open_with(&*keys)
        .await?
        .ok_or_else(|| anyhow::anyhow!("the key of subscriber {} is not readable yet", state.id))?;
    upsert_subscriber(&pool, &event.tenant, &subscriber).await
}

#[evento::subscription_all]
async fn on_campaign_event<E: Executor>(
    context: &Context<'_, E>,
    event: RawEvent<CampaignStream>,
) -> anyhow::Result<()> {
    let pool: SqlitePool = context.extract();
    let Some(state) = campaign_projection()
        .load(&event.aggregate_id)
        .tenant(&event.tenant)
        .execute(context.executor)
        .await?
    else {
        return Ok(());
    };
    upsert_campaign(&pool, &event.tenant, &state).await
}

#[evento::subscription_all]
async fn on_delivery_event<E: Executor>(
    context: &Context<'_, E>,
    event: RawEvent<DeliveryStream>,
) -> anyhow::Result<()> {
    let pool: SqlitePool = context.extract();
    let Some(state) = delivery_projection()
        .load(&event.aggregate_id)
        .tenant(&event.tenant)
        .execute(context.executor)
        .await?
    else {
        return Ok(());
    };
    sqlx::query(
        "INSERT INTO newsletter_deliveries (tenant_id, campaign_id, subscriber_id, failed)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT (tenant_id, campaign_id, subscriber_id) DO UPDATE SET
            failed = excluded.failed",
    )
    .bind(&event.tenant)
    .bind(&state.campaign_id)
    .bind(&state.subscriber_id)
    .bind(state.failed.is_some())
    .execute(&pool)
    .await?;
    Ok(())
}

/// Writes the row of a campaign as the write model sees it.
pub async fn upsert_campaign(
    pool: &SqlitePool,
    tenant_id: &str,
    campaign: &CampaignState,
) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO newsletter_campaigns (tenant_id, id, subject, language, status,
            created_at, sent_at, finished_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
         ON CONFLICT (tenant_id, id) DO UPDATE SET
            subject = excluded.subject, language = excluded.language,
            status = excluded.status, sent_at = excluded.sent_at,
            finished_at = excluded.finished_at, updated_at = excluded.updated_at",
    )
    .bind(tenant_id)
    .bind(&campaign.id)
    .bind(&campaign.subject)
    .bind(&campaign.language)
    .bind(campaign.status.as_str())
    .bind(campaign.created_at as i64)
    .bind(campaign.sent_at.map(|at| at as i64))
    .bind(campaign.finished_at.map(|at| at as i64))
    .bind(campaign.updated_at as i64)
    .execute(pool)
    .await?;
    Ok(())
}

/// Drops the row of an erased subscriber.
pub async fn delete_subscriber(
    pool: &SqlitePool,
    tenant_id: &str,
    subscriber_id: &str,
) -> anyhow::Result<()> {
    sqlx::query("DELETE FROM newsletter_subscribers WHERE tenant_id = ?1 AND id = ?2")
        .bind(tenant_id)
        .bind(subscriber_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Writes the row of a subscriber as the write model sees them, opened.
pub async fn upsert_subscriber(
    pool: &SqlitePool,
    tenant_id: &str,
    subscriber: &Subscriber,
) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO newsletter_subscribers (tenant_id, id, email, language, source, status,
            requested_at, confirmed_at, unsubscribed_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
         ON CONFLICT (tenant_id, id) DO UPDATE SET
            email = excluded.email, language = excluded.language,
            source = excluded.source, status = excluded.status,
            requested_at = excluded.requested_at, confirmed_at = excluded.confirmed_at,
            unsubscribed_at = excluded.unsubscribed_at, updated_at = excluded.updated_at",
    )
    .bind(tenant_id)
    .bind(&subscriber.id)
    .bind(&subscriber.email)
    .bind(&subscriber.language)
    .bind(&subscriber.source)
    .bind(subscriber.status.as_str())
    .bind(subscriber.requested_at as i64)
    .bind(subscriber.confirmed_at.map(|at| at as i64))
    .bind(subscriber.unsubscribed_at.map(|at| at as i64))
    .bind(subscriber.updated_at as i64)
    .execute(pool)
    .await?;
    Ok(())
}
