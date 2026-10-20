//! The subscription that keeps `newsletter_subscribers`,
//! `newsletter_campaigns` and `newsletter_deliveries` current for one tenant.
//!
//! Each handler reloads the strict write model of the stream and rewrites
//! its row (a subscriber's opened with their key), so delivery may repeat (the
//! subscription is at-least-once) without the row drifting; an erased
//! subscriber loses their row. A key not readable yet (on its way from
//! another node) fails the handler, which is retried. It runs on the
//! tenant's own executor (its cursor lives there) and writes into the shared
//! read-model database it is given.

use std::sync::Arc;

use evento::{
    Executor,
    metadata::RawEvent,
    subscription::{Context, SubscriptionBuilder},
};
use sqlx::SqlitePool;
use timada_core::Keys;

use crate::aggregator::{
    Campaign as CampaignStream, Delivery as DeliveryStream, Subscriber as SubscriberStream,
};
use crate::command::{
    CampaignState, Subscriber, campaign_projection, delivery_projection, subscriber_projection,
};

/// The cursor identity of the read-model subscription, in each tenant's store.
pub const READ_MODEL_SUBSCRIPTION: &str = "timada-newsletter/read-models";

/// The tenant whose stream a subscription follows, handed to its handlers.
#[derive(Debug, Clone)]
pub struct TenantId(pub String);

/// Where the tenant's keys are.
#[derive(Clone)]
struct TenantKeys(Arc<dyn Keys>);

/// Builds the subscription of one tenant over every routing key. `pool` is
/// the shared read-model database and must be allowed to write; `keys`
/// opens the addresses. Start it with `.start(&tenant_executor)` and keep
/// the handle for shutdown.
pub fn read_model_subscription<E: Executor>(
    pool: SqlitePool,
    tenant_id: &str,
    keys: Arc<dyn Keys>,
) -> SubscriptionBuilder<E> {
    SubscriptionBuilder::new(READ_MODEL_SUBSCRIPTION)
        .data(pool)
        .data(TenantId(tenant_id.to_owned()))
        .data(TenantKeys(keys))
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
    let tenant: TenantId = context.extract();
    let keys: TenantKeys = context.extract();
    let Some(state) = subscriber_projection()
        .load(&event.aggregate_id)
        .execute(context.executor)
        .await?
    else {
        return Ok(());
    };
    if state.forgotten {
        return delete_subscriber(&pool, &tenant.0, &state.id).await;
    }
    let subscriber = state
        .open_with(&*keys.0)
        .await?
        .ok_or_else(|| anyhow::anyhow!("the key of subscriber {} is not readable yet", state.id))?;
    upsert_subscriber(&pool, &tenant.0, &subscriber).await
}

#[evento::subscription_all]
async fn on_campaign_event<E: Executor>(
    context: &Context<'_, E>,
    event: RawEvent<CampaignStream>,
) -> anyhow::Result<()> {
    let pool: SqlitePool = context.extract();
    let tenant: TenantId = context.extract();
    let Some(state) = campaign_projection()
        .load(&event.aggregate_id)
        .execute(context.executor)
        .await?
    else {
        return Ok(());
    };
    upsert_campaign(&pool, &tenant.0, &state).await
}

#[evento::subscription_all]
async fn on_delivery_event<E: Executor>(
    context: &Context<'_, E>,
    event: RawEvent<DeliveryStream>,
) -> anyhow::Result<()> {
    let pool: SqlitePool = context.extract();
    let tenant: TenantId = context.extract();
    let Some(state) = delivery_projection()
        .load(&event.aggregate_id)
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
    .bind(&tenant.0)
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
