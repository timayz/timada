//! Frees the index entries of the slugs a page let go of: renamed, or
//! deleted.
//!
//! The commands only write what the page did; this subscription gives the
//! slugs back, so a node dying after the command, or a command asked again,
//! never leaves a slug claimed for good, and the old slug of a renamed page
//! stops resolving. Every node runs it at its own pace and delivery may
//! repeat: a release reads the page as it is now and frees only what it
//! holds no more and claimed before the event, so a slug taken back since
//! stays its own. A claim does not wait for it either: a slug whose page let
//! go of it is taken over at once.

use evento::{
    Evento, Executor,
    metadata::Event,
    subscription::{Context, SubscriptionBuilder},
};

use crate::Command;
use crate::aggregator::{Deleted, Renamed};

/// The cursor identity of the subscription, in the store.
pub const RELEASE_SUBSCRIPTION: &str = "timada-pages/releases";

/// The store, to write with (a handler only borrows the executor it runs
/// on).
#[derive(Clone)]
struct Store(Evento);

/// Builds the subscription over every tenant and routing key; `store` is
/// the store it follows, which it frees the entries in (in the tenant of
/// the event).
pub fn release_subscription<E: Executor>(store: Evento) -> SubscriptionBuilder<E> {
    SubscriptionBuilder::new(RELEASE_SUBSCRIPTION)
        .data(Store(store))
        .handler(on_renamed())
        .handler(on_deleted())
        .any_routing_key()
        .retry(5)
}

/// Frees what the page of `event` let go of up to it.
async fn release_page<E: Executor, D>(
    context: &Context<'_, E>,
    event: &Event<'_, D>,
) -> anyhow::Result<()> {
    let store: Store = context.extract();
    Command::new(store.0, &event.tenant)
        .release_given_up(
            &event.aggregate_id,
            event.version,
            event.routing_key.clone(),
        )
        .await?;
    Ok(())
}

#[evento::subscription]
async fn on_renamed<E: Executor>(
    context: &Context<'_, E>,
    event: Event<Renamed>,
) -> anyhow::Result<()> {
    release_page(context, &event).await
}

#[evento::subscription]
async fn on_deleted<E: Executor>(
    context: &Context<'_, E>,
    event: Event<Deleted>,
) -> anyhow::Result<()> {
    release_page(context, &event).await
}
