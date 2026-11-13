//! Settles an order's money with the provider when no page does: takes the
//! held money once it may be, at once for goods the shop holds itself
//! (`Order::Placed` naming no supplier) and once AliExpress took the order of
//! goods bought there (`Forward::Placed`), the goods the shop holds of a
//! mixed order with them; and gives it back when AliExpress closed the order
//! (its `Order::CancellationRequested`). Every node runs it; the node whose
//! request lands on the order is the one that asks for a capture, a
//! cancellation is settled by whichever node gets to it (the provider makes
//! it once), and a provider that could not be reached is asked again by the
//! sweep ([`PendingPayments`](super::PendingPayments)). Replayed, a request
//! finds the money settled, and does nothing.
//!
//! The pass serves every tenant of the region, so only the store's own
//! faults fail it (`saga_fault`). A fault of the order's own is recorded on
//! it (`Order::CaptureFailed`, when this node's request stands; the merchant
//! takes the payment again from the order page) or left to the sweep (a
//! cancellation), and the event given up on, a dead letter (#483).

use std::sync::Arc;

use evento::{
    Evento, Executor,
    metadata::Event,
    subscription::{Context, SubscriptionBuilder},
};
use timada_aliexpress::ALIEXPRESS_ACTOR;
use timada_aliexpress::aggregator::forward::Placed as PlacedOnAliexpress;
use timada_orders::aggregator::{CancellationRequested, Placed};
use timada_orders::{CaptureClaim, OrderError};

use super::{DASHBOARD_ACTOR, Payments, settle};
use crate::saga_fault::{event_fault, fault, recorded};

/// The cursor identity of the subscription, in the store.
pub const ORDER_PAYMENTS_SUBSCRIPTION: &str = "timada-dashboard/order-payments";

/// The store, to write with.
#[derive(Clone)]
struct Store(Evento);

pub fn order_payments_subscription<E: Executor>(
    payments: Arc<Payments>,
    store: Evento,
) -> SubscriptionBuilder<E> {
    SubscriptionBuilder::new(ORDER_PAYMENTS_SUBSCRIPTION)
        .data(payments)
        .data(Store(store))
        .handler(on_placed())
        .handler(on_placed_on_aliexpress())
        .handler(on_cancellation_requested())
        // A saga: it follows the node's own region (the executor's default
        // routing key), so a region's nodes do its work and the other
        // regions' nodes leave it to them.
        .retry(5)
        // An order's own fault gives the event up: a dead letter, and the
        // pass goes on to the other tenants.
        .dead_letter()
        .fault(fault)
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
    let account = match payments.active_tenant(store, tenant_id).await? {
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

/// Settles `outcome` of a step about the order `order_id`: the store's fault
/// fails the pass; the order's own is recorded on it, when this node's
/// capture request stands (`CaptureFailed`: the merchant takes the payment
/// again), and the event given up on either way.
async fn settle_fault<E: Executor>(
    context: &Context<'_, E>,
    event: &evento::Event,
    order_id: Option<&str>,
    outcome: anyhow::Result<()>,
) -> anyhow::Result<()> {
    let Err(err) = outcome else {
        return Ok(());
    };
    let Some(reason) = event_fault(&err) else {
        return Err(err);
    };
    if let Some(order_id) = order_id {
        let store: Store = context.extract();
        let failure = timada_orders::Command::new(store.0, &event.tenant)
            .record_capture_failure(DASHBOARD_ACTOR, order_id, &reason)
            .await
            .map_err(anyhow::Error::from);
        recorded(failure, "the capture failure")?;
    }
    Err(err)
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
    let taken = take_payment(
        &payments,
        &store.0,
        &event.tenant,
        &event.aggregate_id,
        DASHBOARD_ACTOR,
    )
    .await;
    settle_fault(context, &event, Some(&event.aggregate_id), taken).await
}

#[evento::subscription]
async fn on_placed_on_aliexpress<E: Executor>(
    context: &Context<'_, E>,
    event: Event<PlacedOnAliexpress>,
) -> anyhow::Result<()> {
    let payments: Arc<Payments> = context.extract();
    let store: Store = context.extract();
    let Some(forward) = timada_aliexpress::Command::new(store.0.clone(), &event.tenant)
        .load_forward_by_id(&event.aggregate_id)
        .await?
    else {
        return Ok(());
    };
    let taken = take_payment(
        &payments,
        &store.0,
        &event.tenant,
        &forward.order_id,
        ALIEXPRESS_ACTOR,
    )
    .await;
    settle_fault(context, &event, Some(&forward.order_id), taken).await
}

/// A cancellation AliExpress asked for (it closed the order): settled here,
/// as no page waits to settle it. A merchant's or a shopper's is settled by
/// the page that asked.
#[evento::subscription]
async fn on_cancellation_requested<E: Executor>(
    context: &Context<'_, E>,
    event: Event<CancellationRequested>,
) -> anyhow::Result<()> {
    if event.data.requested_by != ALIEXPRESS_ACTOR {
        return Ok(());
    }
    let payments: Arc<Payments> = context.extract();
    let store: Store = context.extract();
    let settled = async {
        let Some(tenant) = payments.active_tenant(&store.0, &event.tenant).await? else {
            return Ok(());
        };
        let orders = timada_orders::Command::new(store.0.clone(), &event.tenant);
        // Nothing held (its hold went back already): settled without the
        // provider, the shop's keys there or not.
        if settle::settle_released_cancellation(&orders, &event.aggregate_id)
            .await?
            .is_some()
        {
            return Ok(());
        }
        let Some(account) = payments.account_of(&tenant)? else {
            return Ok(());
        };
        settle::settle_cancellation(&payments, &account, &orders, &event.aggregate_id).await?;
        anyhow::Ok(())
    };
    // The cancellation stands: the sweep settles it, no capture request to
    // record a failure on.
    settle_fault(context, &event, None, settled.await).await
}
