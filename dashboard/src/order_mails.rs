//! Mails the shopper of every order, under the shop's name, with a link to
//! the order on the shop's storefront when the merchant said where it
//! lives: its confirmation once placed, the payment taken once its invoice
//! is issued (so the page it links to shows it), and its cancellation,
//! saying whether money goes back.

use std::sync::Arc;

use evento::{
    Evento, Executor,
    metadata::Event,
    subscription::{Context, SubscriptionBuilder},
};
use timada_invoicing::aggregator::InvoiceIssued;
use timada_orders::aggregator::{Cancelled, Placed};
use timada_tenancy::ShopMailKind;
use topcoat::context::{AppContext, Cx};

use crate::cluster::Executor as Store;
use crate::mail::{send_order_cancelled, send_order_confirmation, send_order_paid};

/// The cursor identity of the subscription, in the store.
pub const SUBSCRIPTION: &str = "timada-dashboard/order-mails";
/// Who the mails say mailed them.
pub const MAILER: &str = "timada-dashboard";

/// An event older than this is history being replayed (a node catching up,
/// or the first start of this subscription): its shopper is not mailed now.
const MAIL_WINDOW_SECS: u64 = 60 * 60;

/// The platform store, where the shop's name lives.
#[derive(Clone)]
struct Platform(Store);

/// The store, to claim each mail with.
#[derive(Clone)]
struct TenantStore(Evento);

/// What sending needs outside a request: the platform's transport and
/// sender, and the shops' own servers.
#[derive(Clone)]
struct Mail(Arc<AppContext>);

/// Follows every tenant's orders and invoices. Every node runs it; the node
/// that claims an order's mail first is the one that sends it.
pub fn subscription<E: Executor>(
    platform: Store,
    mail: Arc<AppContext>,
    store: Evento,
) -> SubscriptionBuilder<E> {
    SubscriptionBuilder::new(SUBSCRIPTION)
        .data(Platform(platform))
        .data(Mail(mail))
        .data(TenantStore(store))
        .handler(on_placed())
        .handler(on_invoice_issued())
        .handler(on_cancelled())
        .any_routing_key()
        .retry(5)
}

#[evento::subscription]
async fn on_placed<E: Executor>(
    context: &Context<'_, E>,
    event: Event<Placed>,
) -> anyhow::Result<()> {
    mail_order(
        context,
        &event.tenant,
        &event.aggregate_id,
        ShopMailKind::OrderConfirmation,
        event.timestamp,
    )
    .await
}

#[evento::subscription]
async fn on_invoice_issued<E: Executor>(
    context: &Context<'_, E>,
    event: Event<InvoiceIssued>,
) -> anyhow::Result<()> {
    mail_order(
        context,
        &event.tenant,
        &event.data.order_id,
        ShopMailKind::OrderPaid,
        event.timestamp,
    )
    .await
}

#[evento::subscription]
async fn on_cancelled<E: Executor>(
    context: &Context<'_, E>,
    event: Event<Cancelled>,
) -> anyhow::Result<()> {
    mail_order(
        context,
        &event.tenant,
        &event.aggregate_id,
        ShopMailKind::OrderCancelled,
        event.timestamp,
    )
    .await
}

/// Sends the shopper of `order_id` its mail `kind`, unless the event at
/// `at` is history being replayed or another node sent it.
async fn mail_order<E: Executor>(
    context: &Context<'_, E>,
    tenant_id: &str,
    order_id: &str,
    kind: ShopMailKind,
    at: u64,
) -> anyhow::Result<()> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs());
    if now.saturating_sub(at) > MAIL_WINDOW_SECS {
        return Ok(());
    }
    let platform: Platform = context.extract();
    let store: TenantStore = context.extract();
    let orders = timada_orders::Command::new(store.0.clone(), tenant_id);
    // Read whole: what is written with the event (a discount, the shipping)
    // is in the same commit.
    let Some(order) = orders.load_order(order_id).await? else {
        return Ok(());
    };
    let Some(shop) = timada_tenancy::Command(platform.0.clone())
        .load_tenant(tenant_id)
        .await?
    else {
        return Ok(());
    };
    let link = orders
        .storefront_address()
        .await?
        .map(|address| format!("{address}/orders/{}", order.id));
    if !orders
        .claim_order_mail(&order.id, kind.key(), &order.email, MAILER)
        .await?
    {
        return Ok(());
    }
    // Claimed, then sent: a failure here is not retried, since a shopper
    // mailed twice is worse off than one who reads the order's page.
    let mail: Mail = context.extract();
    let cx = Cx::new(mail.0);
    let link = link.as_deref();
    let sent = match kind {
        ShopMailKind::OrderPaid => send_order_paid(&cx, &shop, &order, link).await,
        ShopMailKind::OrderCancelled => send_order_cancelled(&cx, &shop, &order, link).await,
        _ => send_order_confirmation(&cx, &shop, &order, link).await,
    };
    match sent {
        Ok(()) => {
            tracing::info!(tenant = %tenant_id, order = %order.id, mail = kind.key(), "order mail sent");
        }
        Err(err) => {
            tracing::error!(tenant = %tenant_id, order = %order.id, mail = kind.key(), error = %err, "order mail not delivered");
        }
    }
    Ok(())
}
