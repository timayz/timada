//! Frees the index entries of the slugs and stock codes a product or a
//! category let go of: archived, deleted, renamed, a variant's code changed
//! or the variant removed.
//!
//! The commands only write what the product did; this subscription gives
//! the values back, so a node dying after the command, or a command asked
//! again, never leaves a value claimed for good. Every node runs it at its
//! own pace and delivery may repeat: a release reads the product as it is
//! now and frees only what it holds no more and claimed before the event,
//! so a value taken back since stays its own. A claim does not wait for
//! it either: a value whose holder let go of it is taken over at once.

use evento::{
    Evento, Executor,
    metadata::Event,
    subscription::{Context, SubscriptionBuilder},
};

use crate::Command;
use crate::aggregator::category::Archived as CategoryArchived;
use crate::aggregator::{Archived, Deleted, Renamed, VariantRemoved, VariantSkuChanged};

/// The cursor identity of the subscription, in the store.
pub const RELEASE_SUBSCRIPTION: &str = "timada-catalog/releases";

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
        .handler(on_deleted())
        .handler(on_renamed())
        .handler(on_variant_sku_changed())
        .handler(on_variant_removed())
        .handler(on_category_archived())
        .any_routing_key()
        .retry(5)
}

/// Frees what the product of `event` let go of up to it.
async fn release_product<E: Executor, D>(
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
async fn on_archived<E: Executor>(
    context: &Context<'_, E>,
    event: Event<Archived>,
) -> anyhow::Result<()> {
    release_product(context, &event).await
}

#[evento::subscription]
async fn on_deleted<E: Executor>(
    context: &Context<'_, E>,
    event: Event<Deleted>,
) -> anyhow::Result<()> {
    release_product(context, &event).await
}

#[evento::subscription]
async fn on_renamed<E: Executor>(
    context: &Context<'_, E>,
    event: Event<Renamed>,
) -> anyhow::Result<()> {
    release_product(context, &event).await
}

#[evento::subscription]
async fn on_variant_sku_changed<E: Executor>(
    context: &Context<'_, E>,
    event: Event<VariantSkuChanged>,
) -> anyhow::Result<()> {
    release_product(context, &event).await
}

#[evento::subscription]
async fn on_variant_removed<E: Executor>(
    context: &Context<'_, E>,
    event: Event<VariantRemoved>,
) -> anyhow::Result<()> {
    release_product(context, &event).await
}

#[evento::subscription]
async fn on_category_archived<E: Executor>(
    context: &Context<'_, E>,
    event: Event<CategoryArchived>,
) -> anyhow::Result<()> {
    let store: Store = context.extract();
    Command::new(store.0, &event.tenant)
        .release_category_given_up(&event.aggregate_id, event.routing_key.clone())
        .await?;
    Ok(())
}
