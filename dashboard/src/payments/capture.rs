//! Takes an order's held money once it may be: at once for goods the shop
//! holds itself (`Order::Placed` naming no supplier). Every node runs it;
//! the node whose request lands on the order is the one that speaks to the
//! provider, and a provider that could not be reached is asked again by the
//! sweep ([`PendingPayments`](super::PendingPayments)).

use std::sync::Arc;

use evento::{
    Evento, Executor,
    metadata::Event,
    subscription::{Context, SubscriptionBuilder},
};
use timada_orders::aggregator::Placed;
use timada_orders::{CaptureClaim, OrderError};

use super::{DASHBOARD_ACTOR, Payments, settle};

/// The cursor identity of the subscription, in the store.
pub const CAPTURE_SUBSCRIPTION: &str = "timada-dashboard/payment-capture";

/// The store, to write with.
#[derive(Clone)]
struct Store(Evento);

pub fn capture_subscription<E: Executor>(
    payments: Arc<Payments>,
    store: Evento,
) -> SubscriptionBuilder<E> {
    SubscriptionBuilder::new(CAPTURE_SUBSCRIPTION)
        .data(payments)
        .data(Store(store))
        .handler(on_placed())
        .any_routing_key()
        .retry(5)
}

/// Asks for the money of `order_id` and, when this node asked, takes it.
/// An order cancelled meanwhile, or paid already, is nothing to do.
pub async fn take_payment(
    payments: &Payments,
    store: &Evento,
    tenant_id: &str,
    order_id: &str,
    actor: &str,
) -> anyhow::Result<()> {
    let orders = timada_orders::Command::new(store.clone(), tenant_id);
    match orders.request_capture(actor, order_id).await {
        Ok(CaptureClaim::Claimed(_)) => {}
        Ok(CaptureClaim::Pending(_) | CaptureClaim::Paid) => return Ok(()),
        Err(OrderError::OrderCancelled | OrderError::CancellationPending) => return Ok(()),
        Err(err) => return Err(err.into()),
    }
    let account = match payments.active_tenant(tenant_id).await? {
        Some(tenant) => payments.account_of(&tenant)?,
        None => None,
    };
    // A shop suspended, or without its account for now: the request stands
    // for the sweep.
    let Some(account) = account else {
        return Ok(());
    };
    settle::capture(payments, &account, &orders, order_id).await?;
    Ok(())
}

#[evento::subscription]
async fn on_placed<E: Executor>(
    context: &Context<'_, E>,
    event: Event<Placed>,
) -> anyhow::Result<()> {
    if event.data.supplier.is_some() {
        return Ok(());
    }
    let payments: Arc<Payments> = context.extract();
    let store: Store = context.extract();
    take_payment(
        &payments,
        &store.0,
        &event.tenant,
        &event.aggregate_id,
        DASHBOARD_ACTOR,
    )
    .await
}
