//! Erases what a deleted shopper account leaves in the other contexts: the
//! newsletter subscription of the same address.
//!
//! The account names the subscription in its `Forgotten` event (the request
//! read it while the key still opened the address), so the erasure finishes
//! whatever became of the request that deleted the account: a node that
//! stopped right after the account was forgotten, or a newsletter write
//! that failed, no longer leaves an address subscribed that nobody can name
//! again.

use std::sync::Arc;

use evento::{
    Evento, Executor,
    metadata::Event,
    subscription::{Context, SubscriptionBuilder},
};
use timada_core::keys::KeyRing;
use timada_newsletter::NewsletterError;
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
        .any_routing_key()
        .retry(5)
}

#[evento::subscription]
async fn on_forgotten<E: Executor>(
    context: &Context<'_, E>,
    event: Event<Forgotten>,
) -> anyhow::Result<()> {
    let Some(subscriber_id) = &event.data.newsletter_subscriber else {
        return Ok(());
    };
    let TenantStore(store) = context.extract();
    let TenantKeys(keys) = context.extract();
    let erased = timada_newsletter::Command::new(store, &event.tenant)
        .forget_subscriber(&*keys.for_tenant(&event.tenant), ERASER, subscriber_id)
        .await;
    match erased {
        // Erased already, or the address never subscribed.
        Ok(()) | Err(NewsletterError::NotFound) => Ok(()),
        Err(err) => Err(err.into()),
    }
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

    /// The account of `email` forgotten, naming `subscriber`: the event the
    /// saga follows (the account itself is not what it reads).
    async fn forgotten(executor: &Turso, email: &str, subscriber: Option<String>) {
        evento::append(shopper_id(email))
            .original_version(0)
            .event(&Forgotten {
                key_id: "key-1".to_owned(),
                newsletter_subscriber: subscriber,
            })
            .tenant(TENANT)
            .commit(executor)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn a_deleted_account_takes_its_newsletter_subscription_with_it() {
        let executor = Turso::in_memory_concurrent().await.unwrap();
        let vault = vault();
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
                1_800_000_000,
            )
            .await
            .unwrap();
        let subscriber = newsletter.subscriber_of(ADA).await.unwrap().unwrap();
        forgotten(&executor, ADA, Some(subscriber.id.clone())).await;
        // Another account, deleted with no subscription to name.
        forgotten(&executor, "bob@example.com", None).await;

        // Told twice, as every node does: erased once.
        for _ in 0..2 {
            subscription::<Turso>(Evento::new(executor.clone()), vault.ring())
                .run_once(&executor)
                .await
                .unwrap();
        }
        assert!(newsletter.subscriber_of(ADA).await.unwrap().is_none());
    }
}
