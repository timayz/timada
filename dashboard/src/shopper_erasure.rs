//! Erases what a deleted shopper account leaves in the other contexts: the
//! newsletter subscription of the same address.
//!
//! The account names the subscription in its `Forgotten` event (the request
//! read it while the key still opened the address), so the erasure finishes
//! whatever became of the request that deleted the account: a node that
//! stopped right after the account was forgotten, or a newsletter write
//! that failed, no longer leaves an address subscribed that nobody can name
//! again.
//!
//! The event names the subscription by its stream and its key. The address
//! may subscribe again on the same stream, under a new key, and every node
//! hears of the deletion at its own pace (a node behind the others, or one
//! copying its store back, hours later): erasing by key leaves that newer
//! subscription alone.

use std::sync::Arc;

use evento::{
    Evento, Executor,
    metadata::Event,
    subscription::{Context, SubscriptionBuilder},
};
use timada_core::keys::KeyRing;
use timada_shoppers::NewsletterSubscription;
use timada_shoppers::aggregator::Forgotten;

/// The cursor identity of the subscription, in the store.
pub const SUBSCRIPTION: &str = "timada-dashboard/shopper-erasure";

/// Who the newsletter streams say erased the subscription.
const ERASER: &str = "timada-dashboard";

/// The store, to erase the subscription in (a handler only borrows the
/// executor it runs on).
#[derive(Clone)]
struct TenantStore(Evento);

/// The tenants' keys: erasing a subscription destroys its key.
#[derive(Clone)]
struct TenantKeys(Arc<dyn KeyRing>);

/// Follows every tenant's shoppers. Every node runs it; erasing is told
/// twice as harmlessly as once.
pub fn subscription<E: Executor>(store: Evento, keys: Arc<dyn KeyRing>) -> SubscriptionBuilder<E> {
    SubscriptionBuilder::new(SUBSCRIPTION)
        .data(TenantStore(store))
        .data(TenantKeys(keys))
        .handler(on_forgotten())
        // A saga: it follows the node's own region (the executor's default
        // routing key), so a region's nodes do its work and the other
        // regions' nodes leave it to them.
        .retry(5)
}

#[evento::subscription]
async fn on_forgotten<E: Executor>(
    context: &Context<'_, E>,
    event: Event<Forgotten>,
) -> anyhow::Result<()> {
    let Some(subscription) = &event.data.newsletter else {
        return Ok(());
    };
    let TenantStore(store) = context.extract();
    let TenantKeys(keys) = context.extract();
    erase(store, &*keys, &event.tenant, subscription).await
}

/// Erases `subscription` in `tenant`, unless the address subscribed again
/// since (under another key) or it is gone already.
async fn erase(
    store: Evento,
    keys: &dyn KeyRing,
    tenant: &str,
    subscription: &NewsletterSubscription,
) -> anyhow::Result<()> {
    timada_newsletter::Command::new(store, tenant)
        .forget_subscription(
            &*keys.for_tenant(tenant),
            ERASER,
            &subscription.subscriber_id,
            &subscription.key_id,
        )
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use evento::Turso;
    use timada_newsletter::{RequestSubscription, SOURCE_ACCOUNT};
    use timada_shoppers::shopper_id;

    use super::*;
    use crate::config::DataKeysConfig;
    use crate::key_vault::KeyVault;
    use crate::seal::Sealer;

    const TENANT: &str = "acme";
    const ADA: &str = "ada@example.com";

    fn vault() -> Arc<KeyVault> {
        let dir = std::env::temp_dir().join(format!("timada-keys-{}", ulid::Ulid::generate()));
        Arc::new(KeyVault::new(
            &DataKeysConfig { dir },
            Some(Sealer::new(
                "shoppers",
                "a-test-secret-of-thirty-two-chars!!",
            )),
            None,
        ))
    }

    /// Ada asks for the newsletter; the subscription, as an erasure names it.
    async fn subscribe(
        executor: &Turso,
        vault: &Arc<KeyVault>,
        now: u64,
    ) -> NewsletterSubscription {
        let newsletter = timada_newsletter::Command::new(Evento::new(executor.clone()), TENANT);
        newsletter
            .request_subscription(
                &*vault.for_tenant(TENANT),
                "shopper",
                RequestSubscription {
                    email: ADA.to_owned(),
                    language: None,
                    source: SOURCE_ACCOUNT.to_owned(),
                    time_zone: None,
                },
                now,
            )
            .await
            .unwrap();
        let subscriber = newsletter.subscriber_of(ADA).await.unwrap().unwrap();
        NewsletterSubscription {
            subscriber_id: subscriber.id,
            key_id: subscriber.key_id,
        }
    }

    async fn subscribed(executor: &Turso) -> bool {
        timada_newsletter::Command::new(Evento::new(executor.clone()), TENANT)
            .subscriber_of(ADA)
            .await
            .unwrap()
            .is_some()
    }

    #[tokio::test]
    async fn a_deleted_account_takes_its_newsletter_subscription_with_it() {
        let executor = Turso::in_memory_concurrent().await.unwrap();
        let vault = vault();
        let named = subscribe(&executor, &vault, 1_800_000_000).await;
        for (email, newsletter) in [
            (ADA, Some(named)),
            // Another account, deleted with no subscription to name.
            ("bob@example.com", None),
        ] {
            evento::append(shopper_id(email))
                .original_version(0)
                .event(&Forgotten {
                    key_id: "key-1".to_owned(),
                    newsletter,
                })
                .tenant(TENANT)
                .commit(&executor)
                .await
                .unwrap();
        }
        subscription::<Turso>(Evento::new(executor.clone()), vault.ring())
            .run_once(&executor)
            .await
            .unwrap();
        assert!(!subscribed(&executor).await);
    }

    /// Every node erases on its own: told twice, the second is nothing, and
    /// a node hearing of the deletion after Ada subscribed again leaves the
    /// new subscription alone.
    #[tokio::test]
    async fn an_erasure_heard_late_leaves_a_new_subscription_alone() {
        let executor = Turso::in_memory_concurrent().await.unwrap();
        let vault = vault();
        let store = Evento::new(executor.clone());
        let first = subscribe(&executor, &vault, 1_800_000_000).await;
        for _ in 0..2 {
            erase(store.clone(), &*vault.ring(), TENANT, &first)
                .await
                .unwrap();
        }
        assert!(!subscribed(&executor).await);

        let again = subscribe(&executor, &vault, 1_800_000_600).await;
        assert_eq!(again.subscriber_id, first.subscriber_id, "the same stream");
        assert_ne!(again.key_id, first.key_id, "under a new key");
        erase(store.clone(), &*vault.ring(), TENANT, &first)
            .await
            .unwrap();
        assert!(subscribed(&executor).await, "the new subscription stays");
        erase(store, &*vault.ring(), TENANT, &again).await.unwrap();
        assert!(!subscribed(&executor).await);
    }
}
