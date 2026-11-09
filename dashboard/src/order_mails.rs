//! Mails the shopper of every order, under the shop's name, with a link to
//! the order on the shop's storefront when the merchant said where it
//! lives: its confirmation once placed, the payment taken once its invoice
//! is issued (so the page it links to shows it), its shipment with the
//! parcel number and where to follow it, and its cancellation, saying
//! whether money goes back.
//!
//! A shop with no mail server of its own sends through the platform's. That
//! server mails about the orders the checkout placed and no other: an order
//! a `store:write` key forged in the shop's store would have the platform
//! mail any address, so one whose payment the provider does not know as the
//! shop's is passed over (timada #431). A shop's own server is the shop's
//! to spend.

use std::sync::Arc;

use evento::{
    Evento, Executor,
    metadata::Event,
    subscription::{Context, SubscriptionBuilder},
};
use timada_invoicing::aggregator::InvoiceIssued;
use timada_orders::aggregator::{Cancelled, Fulfilled, Placed};
use timada_tenancy::ShopMailKind;
use topcoat::context::{AppContext, Cx, app_context};

use crate::cluster::Executor as Store;
use crate::mail::{
    send_order_cancelled, send_order_confirmation, send_order_paid, send_order_shipped,
};
use crate::payments::{OrderPayment, Payments};
use crate::shop_mail::ShopMail;

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
/// that claims an order's mail first is the one that sends it. `payments`
/// tells the orders the checkout placed, for the mails the platform's
/// server would send.
pub fn subscription<E: Executor>(
    platform: Store,
    mail: Arc<AppContext>,
    payments: Arc<Payments>,
    store: Evento,
) -> SubscriptionBuilder<E> {
    SubscriptionBuilder::new(SUBSCRIPTION)
        .data(Platform(platform))
        .data(Mail(mail))
        .data(payments)
        .data(TenantStore(store))
        .handler(on_placed())
        .handler(on_invoice_issued())
        .handler(on_fulfilled())
        .handler(on_cancelled())
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
async fn on_fulfilled<E: Executor>(
    context: &Context<'_, E>,
    event: Event<Fulfilled>,
) -> anyhow::Result<()> {
    mail_order(
        context,
        &event.tenant,
        &event.aggregate_id,
        ShopMailKind::OrderShipped,
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
    // Money the merchant collected outside timada was never taken from the
    // card: the two settled it between them, and the mail would say
    // otherwise.
    if kind == ShopMailKind::OrderPaid && order.settled_outside.is_some() {
        return Ok(());
    }
    let Some(shop) = timada_tenancy::Command(platform.0.clone())
        .load_tenant(tenant_id)
        .await?
    else {
        return Ok(());
    };
    let mail: Mail = context.extract();
    let cx = Cx::new(mail.0);
    // The platform's server speaks for the orders the checkout placed alone.
    // A provider that does not answer is not an error to retry: a pass that
    // fails for good stops the subscription for every shop, while a mail
    // passed over leaves this shopper the order's page (and the other nodes
    // their own try, none having claimed it).
    if !app_context::<ShopMail>(&cx).has_shop_mails_server(&shop) {
        let payments: Arc<Payments> = context.extract();
        match payments.order_payment(&shop, &order).await {
            Ok(OrderPayment::Held | OrderPayment::Gone) => {}
            Ok(OrderPayment::Unknown) => {
                tracing::warn!(tenant = %tenant_id, order = %order.id, mail = kind.key(), "order mail not sent: the provider knows no payment of this order, and the platform's server would send it");
                return Ok(());
            }
            Err(err) => {
                tracing::error!(tenant = %tenant_id, order = %order.id, mail = kind.key(), error = %err, "order mail not sent: the payment provider did not answer, and the platform's server would send it");
                return Ok(());
            }
        }
    }
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
    let link = link.as_deref();
    let sent = match kind {
        ShopMailKind::OrderPaid => send_order_paid(&cx, &shop, &order, link).await,
        ShopMailKind::OrderShipped => send_order_shipped(&cx, &shop, &order, link).await,
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
