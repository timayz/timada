//! The newsletter requests of the shoppers who ticked the box at checkout
//! ([`subscription`]), and the links of the shops' newsletters.
//!
//! A request is mailed its confirmation link by
//! [`crate::newsletter_confirmations`], whoever recorded it (this
//! subscription, or the storefront's form through `/store/newsletter`). A
//! link opens a page of the shop's storefront
//! (`{storefront}/newsletter/confirm/{token}`), which hands the token back
//! through `/store/newsletter/confirm`; a shop that has not said where its
//! storefront lives cannot have a link mailed.

use evento::{
    Evento, Executor,
    metadata::Event,
    subscription::{Context, SubscriptionBuilder},
};
use std::sync::Arc;
use timada_newsletter::{RequestSubscription, SOURCE_CHECKOUT};
use timada_orders::aggregator::Placed;

use crate::key_vault::KeyVault;

/// The page of the shop's storefront that confirms `token`.
pub fn confirm_link(storefront: &str, token: &str) -> String {
    format!("{storefront}/newsletter/confirm/{token}")
}

/// The page of the shop's storefront that takes the subscriber of `token`
/// off the list.
pub fn unsubscribe_link(storefront: &str, token: &str) -> String {
    format!("{storefront}/newsletter/unsubscribe/{token}")
}

/// The cursor identity of the subscription, in the store.
pub const SUBSCRIPTION: &str = "timada-dashboard/newsletter-opt-ins";
/// Who the checkout's requests say asked.
pub const ACTOR: &str = "timada-dashboard";

/// An order placed longer ago than this is history being replayed (a node
/// catching up, or the first start of this subscription): its shopper is not
/// asked now.
const OPT_IN_WINDOW_SECS: u64 = 60 * 60;

/// The store, where the subscribers are.
#[derive(Clone)]
struct TenantStore(Evento);

#[derive(Clone)]
struct Keys(Arc<KeyVault>);

/// Follows every tenant's orders: the shopper of a paid order who asked for
/// the shop's news at checkout is recorded as asking; the link that confirms
/// it follows from the request. Every node runs it; a request recorded
/// twice is one request.
pub fn subscription<E: Executor>(keys: Arc<KeyVault>, store: Evento) -> SubscriptionBuilder<E> {
    SubscriptionBuilder::new(SUBSCRIPTION)
        .data(Keys(keys))
        .data(TenantStore(store))
        .handler(on_placed())
        // A saga: it follows the node's own region (the executor's default
        // routing key), so a region's nodes do its work and the other
        // regions' nodes leave it to them.
        .retry(5)
}

#[evento::subscription]
async fn on_placed<E: Executor>(
    context: &Context<'_, E>,
    event: Event<Placed>,
) -> anyhow::Result<()> {
    let now = crate::auth::now();
    if now.saturating_sub(event.timestamp) > OPT_IN_WINDOW_SECS {
        return Ok(());
    }
    let store: TenantStore = context.extract();
    let orders = timada_orders::Command::new(store.0.clone(), &event.tenant);
    // Read whole: the opt-in is written with `Placed`, in the same commit.
    let Some(order) = orders.load_order(&event.aggregate_id).await? else {
        return Ok(());
    };
    if !order.newsletter {
        return Ok(());
    }
    if orders.storefront_address().await?.is_none() {
        tracing::warn!(tenant = %event.tenant, order = %order.id, "newsletter opt-in dropped: the shop has no storefront address");
        return Ok(());
    }
    let keys: Keys = context.extract();
    timada_newsletter::Command::new(store.0.clone(), &event.tenant)
        .request_subscription(
            &*keys.0.for_tenant(&event.tenant),
            ACTOR,
            RequestSubscription {
                email: order.email.clone(),
                language: order.language.clone(),
                source: SOURCE_CHECKOUT.to_owned(),
                time_zone: order.time_zone.clone(),
            },
            now,
        )
        .await?;
    tracing::info!(tenant = %event.tenant, order = %order.id, "newsletter opt-in recorded from the checkout");
    Ok(())
}
