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
use timada_core::db::Db;
use timada_core::keys::KeyRing;

use crate::aggregator::{
    Campaign as CampaignStream, Delivery as DeliveryStream, Edition as EditionStream,
    Subscriber as SubscriberStream,
};
use crate::command::{
    CampaignState, EditionState, Subscriber, campaign_projection, delivery_projection,
    edition_projection, subscriber_projection,
};

/// The cursor identity of the read-model subscription, in the store.
pub const READ_MODEL_SUBSCRIPTION: &str = "timada-newsletter/read-models";

/// Where every tenant's keys are.
#[derive(Clone)]
struct Ring(Arc<dyn KeyRing>);

/// Builds the subscription over every tenant and routing key. `db` is the
/// read-model database and must be allowed to write; `keys` holds each
/// tenant's keys, which open the addresses. Start it with `.start(&store)`
/// and keep the handle for shutdown.
pub fn read_model_subscription<E: Executor>(
    db: Db,
    keys: Arc<dyn KeyRing>,
) -> SubscriptionBuilder<E> {
    SubscriptionBuilder::new(READ_MODEL_SUBSCRIPTION)
        .data(db)
        .data(Ring(keys))
        .handler(on_subscriber_event())
        .handler(on_campaign_event())
        .handler(on_delivery_event())
        .handler(on_edition_event())
        .any_routing_key()
        .retry(u8::MAX)
}

#[evento::subscription_all]
async fn on_subscriber_event<E: Executor>(
    context: &Context<'_, E>,
    event: RawEvent<SubscriberStream>,
) -> anyhow::Result<()> {
    let db: Db = context.extract();
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
        return delete_subscriber(&db, &event.tenant, &state.id).await;
    }
    let keys = keys.0.for_tenant(&event.tenant);
    let subscriber = state
        .open_with(&*keys)
        .await?
        .ok_or_else(|| anyhow::anyhow!("the key of subscriber {} is not readable yet", state.id))?;
    upsert_subscriber(&db, &event.tenant, &subscriber).await
}

#[evento::subscription_all]
async fn on_campaign_event<E: Executor>(
    context: &Context<'_, E>,
    event: RawEvent<CampaignStream>,
) -> anyhow::Result<()> {
    let db: Db = context.extract();
    let Some(state) = campaign_projection()
        .load(&event.aggregate_id)
        .tenant(&event.tenant)
        .execute(context.executor)
        .await?
    else {
        return Ok(());
    };
    upsert_campaign(&db, &event.tenant, &state).await
}

#[evento::subscription_all]
async fn on_delivery_event<E: Executor>(
    context: &Context<'_, E>,
    event: RawEvent<DeliveryStream>,
) -> anyhow::Result<()> {
    let db: Db = context.extract();
    let Some(state) = delivery_projection()
        .load(&event.aggregate_id)
        .tenant(&event.tenant)
        .execute(context.executor)
        .await?
    else {
        return Ok(());
    };
    db.execute(
        "INSERT INTO newsletter_deliveries
            (tenant_id, campaign_id, edition_id, subscriber_id, failed, skipped)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT (tenant_id, edition_id, subscriber_id) DO UPDATE SET
            failed = excluded.failed, skipped = excluded.skipped",
        (
            event.tenant.as_str(),
            state.campaign_id.as_str(),
            state.edition_id.as_deref().unwrap_or(&state.campaign_id),
            state.subscriber_id.as_str(),
            state.failed.is_some(),
            state.skipped.is_some(),
        ),
    )
    .await?;
    Ok(())
}

#[evento::subscription_all]
async fn on_edition_event<E: Executor>(
    context: &Context<'_, E>,
    event: RawEvent<EditionStream>,
) -> anyhow::Result<()> {
    let db: Db = context.extract();
    let Some(state) = edition_projection()
        .load(&event.aggregate_id)
        .tenant(&event.tenant)
        .execute(context.executor)
        .await?
    else {
        return Ok(());
    };
    upsert_edition(&db, &event.tenant, &state).await
}

/// Writes the row of an edition as the write model sees it.
pub async fn upsert_edition(
    db: &Db,
    tenant_id: &str,
    edition: &EditionState,
) -> anyhow::Result<()> {
    db.execute(
        "INSERT INTO newsletter_editions (tenant_id, id, campaign_id, cycle, status,
            opened_at, closed_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT (tenant_id, id) DO UPDATE SET
            cycle = excluded.cycle, status = excluded.status,
            opened_at = excluded.opened_at, closed_at = excluded.closed_at",
        (
            tenant_id,
            edition.id.as_str(),
            edition.campaign_id.as_str(),
            edition.cycle.as_str(),
            if edition.is_open() { "open" } else { "closed" },
            edition.opened_at as i64,
            edition.closed_at.map(|at| at as i64),
        ),
    )
    .await?;
    Ok(())
}

/// Writes the row of a campaign as the write model sees it.
pub async fn upsert_campaign(
    db: &Db,
    tenant_id: &str,
    campaign: &CampaignState,
) -> anyhow::Result<()> {
    db.execute(
        "INSERT INTO newsletter_campaigns (tenant_id, id, subject, language, status,
            created_at, sent_at, finished_at, updated_at, cadence)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
         ON CONFLICT (tenant_id, id) DO UPDATE SET
            subject = excluded.subject, language = excluded.language,
            cadence = excluded.cadence,
            status = excluded.status, sent_at = excluded.sent_at,
            finished_at = excluded.finished_at, updated_at = excluded.updated_at",
        (
            tenant_id,
            campaign.id.as_str(),
            campaign.subject.as_str(),
            campaign.language.as_deref(),
            campaign.status.as_str(),
            campaign.created_at as i64,
            campaign.sent_at.map(|at| at as i64),
            campaign.finished_at.map(|at| at as i64),
            campaign.updated_at as i64,
            campaign
                .schedule
                .as_ref()
                .map(|schedule| schedule.cadence.as_str()),
        ),
    )
    .await?;
    Ok(())
}

/// Drops the row of an erased subscriber.
pub async fn delete_subscriber(
    db: &Db,
    tenant_id: &str,
    subscriber_id: &str,
) -> anyhow::Result<()> {
    db.execute(
        "DELETE FROM newsletter_subscribers WHERE tenant_id = ?1 AND id = ?2",
        (tenant_id, subscriber_id),
    )
    .await?;
    Ok(())
}

/// Writes the row of a subscriber as the write model sees them, opened.
pub async fn upsert_subscriber(
    db: &Db,
    tenant_id: &str,
    subscriber: &Subscriber,
) -> anyhow::Result<()> {
    db.execute(
        "INSERT INTO newsletter_subscribers (tenant_id, id, email, language, source, status,
            requested_at, confirmed_at, unsubscribed_at, updated_at, time_zone)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
         ON CONFLICT (tenant_id, id) DO UPDATE SET
            email = excluded.email, language = excluded.language,
            time_zone = excluded.time_zone,
            source = excluded.source, status = excluded.status,
            requested_at = excluded.requested_at, confirmed_at = excluded.confirmed_at,
            unsubscribed_at = excluded.unsubscribed_at, updated_at = excluded.updated_at",
        (
            tenant_id,
            subscriber.id.as_str(),
            subscriber.email.as_str(),
            subscriber.language.as_deref(),
            subscriber.source.as_str(),
            subscriber.status.as_str(),
            subscriber.requested_at as i64,
            subscriber.confirmed_at.map(|at| at as i64),
            subscriber.unsubscribed_at.map(|at| at as i64),
            subscriber.updated_at as i64,
            subscriber.time_zone.as_deref(),
        ),
    )
    .await?;
    Ok(())
}
