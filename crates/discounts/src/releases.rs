//! Frees the index entry of the code of an archived discount.
//!
//! Archiving only writes `Archived`; this subscription gives the code back,
//! so a node dying after the command, or a command asked again, never
//! leaves a code claimed for good. Every node runs it at its own pace and
//! delivery may repeat: an entry already free, or claimed by another
//! discount since, is left alone. A claim does not wait for it either: the
//! code of an archived discount is taken over at once.

use evento::{
    Evento, Executor,
    metadata::Event,
    subscription::{Context, SubscriptionBuilder},
};

use crate::Command;
use crate::aggregator::Archived;

/// The cursor identity of the subscription, in the store.
pub const RELEASE_SUBSCRIPTION: &str = "timada-discounts/releases";

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
        .handler(on_archived())
        .any_routing_key()
        .retry(5)
}

#[evento::subscription]
async fn on_archived<E: Executor>(
    context: &Context<'_, E>,
    event: Event<Archived>,
) -> anyhow::Result<()> {
    let store: Store = context.extract();
    Command::new(store.0, &event.tenant)
        .release_given_up(&event.aggregate_id, event.routing_key.clone())
        .await?;
    Ok(())
}
