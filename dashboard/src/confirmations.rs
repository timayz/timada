//! Mails the shopper of every paid order its confirmation, under the shop's
//! name, with a link to the order on the shop's storefront when the
//! merchant said where it lives.

use std::sync::Arc;

use evento::{
    Evento, Executor,
    metadata::Event,
    subscription::{Context, SubscriptionBuilder},
};
use timada_orders::aggregator::Placed;
use topcoat::context::{AppContext, Cx};

use crate::auth::mail::send_order_confirmation;
use crate::cluster::Executor as Store;

/// The cursor identity of the subscription, in each tenant's store.
pub const SUBSCRIPTION: &str = "timada-dashboard/order-confirmations";
/// Who the confirmations say mailed them.
pub const MAILER: &str = "timada-dashboard";

/// An order placed longer ago than this is history being replayed (a node
/// catching up, or the first start of this subscription): its shopper is not
/// mailed now.
const CONFIRMATION_WINDOW_SECS: u64 = 60 * 60;

#[derive(Clone)]
struct Tenant(String);

/// The platform store, where the shop's name lives.
#[derive(Clone)]
struct Platform(Store);

/// The tenant's own executor, to claim each confirmation with.
#[derive(Clone)]
struct TenantStore(Evento);

/// What sending needs outside a request: the platform's transport and
/// sender, and the shops' own servers.
#[derive(Clone)]
struct Mail(Arc<AppContext>);

/// Follows one tenant's orders. Every node with the tenant open runs it; the
/// node that claims an order's confirmation first is the one that sends it.
pub fn subscription<E: Executor>(
    platform: Store,
    mail: Arc<AppContext>,
    tenant_id: &str,
    store: Evento,
) -> SubscriptionBuilder<E> {
    SubscriptionBuilder::new(SUBSCRIPTION)
        .data(Platform(platform))
        .data(Mail(mail))
        .data(TenantStore(store))
        .data(Tenant(tenant_id.to_owned()))
        .handler(on_placed())
        .any_routing_key()
        .retry(5)
}

#[evento::subscription]
async fn on_placed<E: Executor>(
    context: &Context<'_, E>,
    event: Event<Placed>,
) -> anyhow::Result<()> {
    let tenant: Tenant = context.extract();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs());
    if now.saturating_sub(event.timestamp) > CONFIRMATION_WINDOW_SECS {
        return Ok(());
    }
    let platform: Platform = context.extract();
    let store: TenantStore = context.extract();
    let orders = timada_orders::Command(store.0.clone());
    // Read whole: a discount is written with `Placed`, in the same commit.
    let Some(order) = orders.load_order(&event.aggregate_id).await? else {
        return Ok(());
    };
    let Some(shop) = timada_tenancy::Command(platform.0.clone())
        .load_tenant(&tenant.0)
        .await?
    else {
        return Ok(());
    };
    let link = orders
        .storefront_address()
        .await?
        .map(|address| format!("{address}/orders/{}", order.id));
    if !orders
        .claim_confirmation(&order.id, &order.email, MAILER)
        .await?
    {
        return Ok(());
    }
    // Claimed, then sent: a failure here is not retried, since a shopper
    // mailed twice is worse off than one left with the provider's receipt.
    let mail: Mail = context.extract();
    let cx = Cx::new(mail.0);
    match send_order_confirmation(&cx, &shop, &order, link.as_deref()).await {
        Ok(()) => {
            tracing::info!(tenant = %tenant.0, order = %order.id, "order confirmation mailed");
        }
        Err(err) => {
            tracing::error!(tenant = %tenant.0, order = %order.id, error = %err, "order confirmation not delivered");
        }
    }
    Ok(())
}
