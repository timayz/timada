//! Starts the runs: every event that is a trigger starts a run of each
//! published scenario of the shop listening to it. The nodes of the event's
//! region all run this subscription and share the runs out: each starts the
//! ones whose id falls to it among them ([`timada_core::Share`]), with one
//! write and no read, and keeps the others in `automations_starts`, where
//! the runner starts any a node has not started a while later (it stopped,
//! or the nodes disagreed on who is up). A run's id is derived from the
//! scenario and the event and written at version 0, so it starts once
//! however many nodes try.
//!
//! The listeners are kept by the same subscription, from the scenarios' own
//! events ([`timada_automations::listener_subscription`]): the order of the
//! store decides which events a publication sees.

use crate::cluster::ShareOf;
use crate::db::Db;
use evento::{
    Evento, Executor,
    metadata::Event,
    subscription::{Context, SubscriptionBuilder},
};
use timada_automations::{
    AUTOMATION_ACTOR, StartRun, catalog, listener_subscription, listeners_for, owe_start,
    record_checked_out, record_checkout, record_detected, run_id,
};

/// An event older than this is history being replayed (a node catching up,
/// or the first start of this subscription): it starts nothing.
pub const TRIGGER_WINDOW_SECS: u64 = 60 * 60;

/// The store, to start the runs in.
#[derive(Clone)]
struct TenantStore(Evento);

/// This node's place among the nodes up of a region: which runs it starts.
#[derive(Clone)]
struct Place(ShareOf);

/// Follows every tenant's trigger events. `db` is the read-model
/// database, which must be allowed to write (the listeners live there);
/// `place` says which of the runs an event of a region starts are this
/// node's to start.
pub fn subscription<E: Executor>(db: Db, store: Evento, place: ShareOf) -> SubscriptionBuilder<E> {
    listener_subscription(db)
        .data(TenantStore(store))
        .data(Place(place))
        .handler(on_order_placed())
        .handler(on_order_fulfilled())
        .handler(on_order_cancelled())
        .handler(on_order_refunded())
        .handler(on_shopper_registered())
        .handler(on_subscriber_confirmed())
        .handler(on_review_submitted())
        .handler(on_review_published())
        .handler(on_message_received())
        .handler(on_checkout_requested())
        .handler(on_cart_checked_out())
        .handler(on_checkout_abandoned())
        // A saga: it follows the node's own region (the executor's default
        // routing key), so a region's nodes do its work and the other
        // regions' nodes leave it to them.
        .retry(5)
}

/// Starts a run of every scenario of the tenant listening to `trigger`:
/// the ones that fall to this node among the nodes of `region` (the
/// event's routing key) now, the others kept for the runner to start if
/// their node does not. Only a store failure is an error: it is retried.
async fn start<E: Executor>(
    context: &Context<'_, E>,
    tenant: &str,
    trigger: &str,
    event_id: String,
    subject_id: &str,
    at: u64,
    region: Option<&str>,
) -> anyhow::Result<()> {
    if crate::auth::now().saturating_sub(at) > TRIGGER_WINDOW_SECS {
        return Ok(());
    }
    let db: Db = context.extract();
    let listeners = listeners_for(&db, tenant, trigger).await?;
    if listeners.is_empty() {
        return Ok(());
    }
    let store: TenantStore = context.extract();
    let place: Place = context.extract();
    let share = (place.0)(region);
    let automations = timada_automations::Command::new(store.0.clone(), tenant);
    for listener in listeners {
        // Published after the event (the same second, in another order).
        if (at as i64) < listener.since {
            continue;
        }
        let start = StartRun {
            scenario_id: listener.scenario_id.clone(),
            scenario_version: listener.version as u32,
            trigger: trigger.to_owned(),
            trigger_event_id: event_id.clone(),
            subject_id: subject_id.to_owned(),
        };
        let run = run_id(&start.scenario_id, &start.trigger_event_id);
        if !share.owns(&run) {
            owe_start(&db, tenant, &run, &start, crate::auth::now()).await?;
            continue;
        }
        let started = automations.start_run(AUTOMATION_ACTOR, &start).await?;
        if started {
            tracing::info!(tenant, scenario = %listener.scenario_id, trigger, "automation run started");
        }
    }
    Ok(())
}

#[evento::subscription]
async fn on_order_placed<E: Executor>(
    context: &Context<'_, E>,
    event: Event<timada_orders::aggregator::Placed>,
) -> anyhow::Result<()> {
    start(
        context,
        &event.tenant,
        catalog::ORDER_PLACED,
        event.id.to_string(),
        &event.aggregate_id,
        event.timestamp,
        event.routing_key.as_deref(),
    )
    .await
}

#[evento::subscription]
async fn on_order_fulfilled<E: Executor>(
    context: &Context<'_, E>,
    event: Event<timada_orders::aggregator::Fulfilled>,
) -> anyhow::Result<()> {
    start(
        context,
        &event.tenant,
        catalog::ORDER_FULFILLED,
        event.id.to_string(),
        &event.aggregate_id,
        event.timestamp,
        event.routing_key.as_deref(),
    )
    .await
}

#[evento::subscription]
async fn on_order_cancelled<E: Executor>(
    context: &Context<'_, E>,
    event: Event<timada_orders::aggregator::Cancelled>,
) -> anyhow::Result<()> {
    start(
        context,
        &event.tenant,
        catalog::ORDER_CANCELLED,
        event.id.to_string(),
        &event.aggregate_id,
        event.timestamp,
        event.routing_key.as_deref(),
    )
    .await
}

#[evento::subscription]
async fn on_order_refunded<E: Executor>(
    context: &Context<'_, E>,
    event: Event<timada_orders::aggregator::Refunded>,
) -> anyhow::Result<()> {
    start(
        context,
        &event.tenant,
        catalog::ORDER_REFUNDED,
        event.id.to_string(),
        &event.aggregate_id,
        event.timestamp,
        event.routing_key.as_deref(),
    )
    .await
}

#[evento::subscription]
async fn on_shopper_registered<E: Executor>(
    context: &Context<'_, E>,
    event: Event<timada_shoppers::aggregator::Registered>,
) -> anyhow::Result<()> {
    start(
        context,
        &event.tenant,
        catalog::SHOPPER_REGISTERED,
        event.id.to_string(),
        &event.aggregate_id,
        event.timestamp,
        event.routing_key.as_deref(),
    )
    .await
}

#[evento::subscription]
async fn on_subscriber_confirmed<E: Executor>(
    context: &Context<'_, E>,
    event: Event<timada_newsletter::aggregator::Confirmed>,
) -> anyhow::Result<()> {
    start(
        context,
        &event.tenant,
        catalog::SUBSCRIBER_CONFIRMED,
        event.id.to_string(),
        &event.aggregate_id,
        event.timestamp,
        event.routing_key.as_deref(),
    )
    .await
}

#[evento::subscription]
async fn on_review_submitted<E: Executor>(
    context: &Context<'_, E>,
    event: Event<timada_reviews::aggregator::Submitted>,
) -> anyhow::Result<()> {
    start(
        context,
        &event.tenant,
        catalog::REVIEW_SUBMITTED,
        event.id.to_string(),
        &event.aggregate_id,
        event.timestamp,
        event.routing_key.as_deref(),
    )
    .await
}

#[evento::subscription]
async fn on_review_published<E: Executor>(
    context: &Context<'_, E>,
    event: Event<timada_reviews::aggregator::Published>,
) -> anyhow::Result<()> {
    start(
        context,
        &event.tenant,
        catalog::REVIEW_PUBLISHED,
        event.id.to_string(),
        &event.aggregate_id,
        event.timestamp,
        event.routing_key.as_deref(),
    )
    .await
}

#[evento::subscription]
async fn on_message_received<E: Executor>(
    context: &Context<'_, E>,
    event: Event<timada_support::aggregator::Received>,
) -> anyhow::Result<()> {
    start(
        context,
        &event.tenant,
        catalog::MESSAGE_RECEIVED,
        event.id.to_string(),
        &event.aggregate_id,
        event.timestamp,
        event.routing_key.as_deref(),
    )
    .await
}

/// A cart asked to be paid for: the runner finds it left once no order
/// followed within the delay.
#[evento::subscription]
async fn on_checkout_requested<E: Executor>(
    context: &Context<'_, E>,
    event: Event<timada_orders::aggregator::CheckoutRequested>,
) -> anyhow::Result<()> {
    let db: Db = context.extract();
    record_checkout(
        &db,
        &event.tenant,
        &event.aggregate_id,
        &event.data.request_id,
        event.timestamp,
        event.routing_key.as_deref(),
    )
    .await
}

#[evento::subscription]
async fn on_cart_checked_out<E: Executor>(
    context: &Context<'_, E>,
    event: Event<timada_orders::aggregator::CheckedOut>,
) -> anyhow::Result<()> {
    let db: Db = context.extract();
    record_checked_out(&db, &event.tenant, &event.aggregate_id).await
}

#[evento::subscription]
async fn on_checkout_abandoned<E: Executor>(
    context: &Context<'_, E>,
    event: Event<timada_automations::aggregator::abandoned::Detected>,
) -> anyhow::Result<()> {
    let db: Db = context.extract();
    record_detected(
        &db,
        &event.tenant,
        &event.data.cart_id,
        &event.data.request_id,
    )
    .await?;
    start(
        context,
        &event.tenant,
        catalog::CHECKOUT_ABANDONED,
        event.id.to_string(),
        &event.data.cart_id,
        event.timestamp,
        event.routing_key.as_deref(),
    )
    .await
}
