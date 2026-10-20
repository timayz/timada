//! Follows each tenant's newsletter subscribers to keep this node's copy of their
//! keys ([`crate::key_vault`]) in step: the key of a new subscriber is fetched,
//! the key of an erased one destroyed.

use std::sync::Arc;

use evento::subscription::{Context, SubscriptionBuilder};
use evento::{Executor, metadata::Event};
use timada_newsletter::aggregator::{Forgotten, Requested};
use timada_newsletter::subscriber_projection;

use crate::key_vault::KeyVault;

/// The cursor identity of the subscription that keeps this node's keys in
/// step with the subscribers, in each tenant's store.
pub const SUBSCRIPTION: &str = "timada-dashboard/newsletter-keys";

/// Follows one tenant's subscribers: fetches the key of each subscriber this node
/// lacks, and destroys the key of each one erased. A fetch that fails is
/// retried, with a growing delay; the key of a subscriber erased since is
/// skipped.
pub fn subscription<E: Executor>(vault: Arc<KeyVault>, tenant_id: &str) -> SubscriptionBuilder<E> {
    SubscriptionBuilder::new(SUBSCRIPTION)
        .data(vault)
        .data(Tenant(tenant_id.to_owned()))
        .handler(on_requested())
        .handler(on_forgotten())
        .any_routing_key()
        // A key is on a majority of nodes before it is recorded, so a fetch
        // only fails while most of the cluster is down.
        .retry(u8::MAX)
}

#[derive(Clone)]
struct Tenant(String);

#[evento::subscription]
async fn on_requested<E: Executor>(
    context: &Context<'_, E>,
    event: Event<Requested>,
) -> anyhow::Result<()> {
    let vault: Arc<KeyVault> = context.extract();
    let tenant: Tenant = context.extract();
    let still_used = subscriber_projection()
        .load(&event.aggregate_id)
        .execute(context.executor)
        .await?
        .is_some_and(|subscriber| !subscriber.forgotten && subscriber.key_id == event.data.key_id);
    if !still_used {
        return Ok(());
    }
    vault
        .for_tenant(&tenant.0)
        .get(&event.data.key_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("no node holds subscriber key {}", event.data.key_id))?;
    Ok(())
}

#[evento::subscription]
async fn on_forgotten<E: Executor>(
    context: &Context<'_, E>,
    event: Event<Forgotten>,
) -> anyhow::Result<()> {
    let vault: Arc<KeyVault> = context.extract();
    let tenant: Tenant = context.extract();
    vault.destroy(&tenant.0, &event.data.key_id).await
}
