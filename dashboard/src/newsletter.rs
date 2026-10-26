//! The confirmation links of the shops' newsletters: mailed when a shopper
//! asks from the storefront's form (`/store/newsletter`), and when an order
//! is paid whose shopper ticked the box at checkout ([`subscription`]).
//!
//! A link opens a page of the shop's storefront
//! (`{storefront}/newsletter/confirm/{token}`), which hands the token back
//! through `/store/newsletter/confirm`; a shop that has not said where its
//! storefront lives cannot have a link mailed.

use evento::{
    Evento, Executor,
    metadata::Event,
    subscription::{Context, SubscriptionBuilder},
};
use std::sync::Arc;
use timada_newsletter::{RequestOutcome, RequestSubscription, SOURCE_CHECKOUT};
use timada_orders::aggregator::Placed;
use timada_tenancy::TenantState;
use topcoat::context::{AppContext, Cx};

use crate::auth::mail::send_newsletter_confirmation;
use crate::cluster::Executor as Store;
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

/// Mails the link of a request just recorded, when there is one to mail.
/// Returns whether a mail went out.
pub async fn mail_confirmation(
    cx: &Cx,
    shop: &TenantState,
    storefront: &str,
    outcome: &RequestOutcome,
) -> topcoat::Result<bool> {
    let RequestOutcome::ToConfirm {
        email,
        language,
        token,
        ..
    } = outcome
    else {
        return Ok(false);
    };
    send_newsletter_confirmation(
        cx,
        shop,
        email,
        &confirm_link(storefront, token),
        language.as_deref(),
    )
    .await?;
    Ok(true)
}

/// The cursor identity of the subscription, in the store.
pub const SUBSCRIPTION: &str = "timada-dashboard/newsletter-opt-ins";
/// Who the checkout's requests say asked.
pub const ACTOR: &str = "timada-dashboard";

/// An order placed longer ago than this is history being replayed (a node
/// catching up, or the first start of this subscription): its shopper is not
/// mailed now.
const OPT_IN_WINDOW_SECS: u64 = 60 * 60;

/// The platform store, where the shop lives.
#[derive(Clone)]
struct Platform(Store);

/// The store, where the subscribers are.
#[derive(Clone)]
struct TenantStore(Evento);

/// What sending needs outside a request.
#[derive(Clone)]
struct Mail(Arc<AppContext>);

#[derive(Clone)]
struct Keys(Arc<KeyVault>);

/// Follows every tenant's orders: the shopper of a paid order who asked for
/// the shop's news at checkout is recorded as asking, and mailed the link
/// that confirms it. Every node runs it; the node whose
/// request is recorded first is the one that mails.
pub fn subscription<E: Executor>(
    platform: Store,
    mail: Arc<AppContext>,
    keys: Arc<KeyVault>,
    store: Evento,
) -> SubscriptionBuilder<E> {
    SubscriptionBuilder::new(SUBSCRIPTION)
        .data(Platform(platform))
        .data(Mail(mail))
        .data(Keys(keys))
        .data(TenantStore(store))
        .handler(on_placed())
        .any_routing_key()
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
    let Some(storefront) = orders.storefront_address().await? else {
        tracing::warn!(tenant = %event.tenant, order = %order.id, "newsletter opt-in dropped: the shop has no storefront address");
        return Ok(());
    };
    let platform: Platform = context.extract();
    let Some(shop) = timada_tenancy::Command(platform.0.clone())
        .load_tenant(&event.tenant)
        .await?
    else {
        return Ok(());
    };
    let keys: Keys = context.extract();
    let outcome = timada_newsletter::Command::new(store.0.clone(), &event.tenant)
        .request_subscription(
            &*keys.0.for_tenant(&event.tenant),
            ACTOR,
            RequestSubscription {
                email: order.email.clone(),
                language: order.language.clone(),
                source: SOURCE_CHECKOUT.to_owned(),
            },
            now,
        )
        .await?;
    // Recorded, then mailed: a failure here is not retried, since the
    // shopper may ask again from the storefront.
    let mail: Mail = context.extract();
    let cx = Cx::new(mail.0);
    match mail_confirmation(&cx, &shop, &storefront, &outcome).await {
        Ok(true) => {
            tracing::info!(tenant = %event.tenant, order = %order.id, "newsletter confirmation mailed");
        }
        Ok(false) => {}
        Err(err) => {
            tracing::error!(tenant = %event.tenant, order = %order.id, error = %err, "newsletter confirmation not delivered");
        }
    }
    Ok(())
}
