//! The order-fulfillment saga: one orchestrator, four aggregates.
//!
//! ```text
//! OrderPlaced             → request_charge                    (appends nothing)
//! ChargeCaptured          → forward_order, per supplier       → OrderPaid
//! ChargeFailed            → (nothing was captured)            → OrderCancelled
//! SupplierOrderConfirmed  → create_shipment                   → OrderForwardedToSupplier
//! SupplierOrderRejected   → refund   ← compensation           → OrderCancelled
//! ShipmentDispatched      → —                                 → OrderShipped
//! ShipmentDelivered       → —                                 → OrderDelivered
//! ```
//!
//! The `Order` aggregate doubles as the saga's state. Its
//! [`OrderStatus`](crate::view::OrderStatus) *is* the saga's position, so there
//! is no second state machine that could drift out of sync with the order the
//! customer is looking at.
//!
//! **Why this subscription is not `.strict()`.** It deliberately handles a
//! subset of four aggregates' events. Without strict mode evento derives the
//! read filters from the registered handlers, so the subscription only ever
//! fetches the seven events below — turning strict on would instead pull in
//! every event of those four aggregate types and then fail on the ones it has
//! no handler for.
//!
//! **Idempotency.** Handlers re-run: a retry resumes from the last acknowledged
//! event, and a failure mid-handler replays the whole handler. Three things
//! make that safe. Every command it dispatches is idempotent on a
//! deterministically derived id (`request_charge`, `forward_order`,
//! `create_shipment`, `refund`). Every append goes through a freshly loaded
//! [`OrderView`] and [`ProjectionAggregate::write`], which carries the replayed
//! version into the optimistic-concurrency check — so a concurrent append makes
//! this one fail and the retry re-decides against fresh state instead of
//! writing a stale event. And every append is guarded on the status ranking, so
//! a second pass over an already-handled event appends nothing.

use std::collections::BTreeMap;
use std::sync::Arc;

use evento::ProjectionAggregate as _;
use evento::metadata::Event;
use evento::subscription::{Context, SubscriptionBuilder};
use timada_core::Executor;
use timada_dropship::{
    SupplierLine, SupplierOrderConfirmed, SupplierOrderRejected, SupplierRegistry, forward_order,
    load_supplier_order,
};
use timada_payment::{
    ChargeCaptured, ChargeFailed, PaymentProvider, load_payment, refund, request_charge,
};
use timada_shipping::{ShipmentDelivered, ShipmentDispatched, create_shipment, load_shipment};

use crate::aggregate::{
    OrderCancelled, OrderDelivered, OrderForwardedToSupplier, OrderLine, OrderPaid, OrderPlaced,
    OrderShipped,
};
use crate::view::{OrderStatus, load_order};

/// Subscription key; also the cursor row's key in evento's `subscriber` table.
pub const FULFILLMENT_SUBSCRIPTION: &str = "order-fulfillment";

/// The saga, unstarted.
///
/// Handlers must be generic over the executor (the macro requires it) but the
/// commands they dispatch need the concrete framework [`Executor`], so it is
/// injected as subscription data alongside the supplier registry and the
/// payment provider.
pub fn fulfillment_subscription(
    executor: Executor,
    registry: SupplierRegistry,
    provider: Arc<dyn PaymentProvider>,
) -> SubscriptionBuilder<Executor> {
    SubscriptionBuilder::<Executor>::new(FULFILLMENT_SUBSCRIPTION)
        .data(executor)
        .data(registry)
        .data(provider)
        .handler(on_order_placed())
        .handler(on_charge_captured())
        .handler(on_charge_failed())
        .handler(on_supplier_order_confirmed())
        .handler(on_supplier_order_rejected())
        .handler(on_shipment_dispatched())
        .handler(on_shipment_delivered())
}

fn missing(what: &str) -> anyhow::Error {
    anyhow::anyhow!("`{FULFILLMENT_SUBSCRIPTION}` was started without {what}")
}

fn executor<E: evento::Executor>(ctx: &Context<'_, E>) -> anyhow::Result<Executor> {
    ctx.get::<Executor>().ok_or_else(|| missing("an executor"))
}

fn registry<E: evento::Executor>(ctx: &Context<'_, E>) -> anyhow::Result<SupplierRegistry> {
    ctx.get::<SupplierRegistry>()
        .ok_or_else(|| missing("a supplier registry"))
}

fn provider<E: evento::Executor>(ctx: &Context<'_, E>) -> anyhow::Result<Arc<dyn PaymentProvider>> {
    ctx.get::<Arc<dyn PaymentProvider>>()
        .ok_or_else(|| missing("a payment provider"))
}

/// Split an order's lines by who fulfills them.
///
/// `BTreeMap` so a replay forwards to the same suppliers in the same order —
/// with a partial failure that means the retry redoes the already-forwarded
/// suppliers (a no-op) before reaching the one that failed.
fn by_supplier(lines: &[OrderLine]) -> BTreeMap<String, Vec<SupplierLine>> {
    let mut groups: BTreeMap<String, Vec<SupplierLine>> = BTreeMap::new();

    for line in lines {
        groups
            .entry(line.supplier_id.clone())
            .or_default()
            .push(SupplierLine {
                supplier_product_ref: line.supplier_product_ref.clone(),
                title: line.title.clone(),
                quantity: line.quantity,
            });
    }

    groups
}

/// Step 1 — ask the payment context to charge the customer.
///
/// Nothing is appended to the order here: the payment's own events are what
/// drive the next step, so a charge that is still in flight cannot be mistaken
/// for a paid order.
#[evento::subscription]
async fn on_order_placed<E: evento::Executor>(
    ctx: &Context<'_, E>,
    event: Event<OrderPlaced>,
) -> anyhow::Result<()> {
    let executor = executor(ctx)?;
    let payment_id = request_charge(
        &executor,
        &provider(ctx)?,
        &event.aggregate_id,
        event.data.total,
    )
    .await?;

    tracing::info!(order_id = %event.aggregate_id, %payment_id, "charge requested for order");
    Ok(())
}

/// Step 2 — record the payment, then hand the lines to their suppliers.
///
/// Forwarding runs on every pass, not only on the pass that appended
/// `OrderPaid`: a partial failure (two suppliers, the second one errors) must
/// resume and finish the rest, and `forward_order` is idempotent per
/// `(order, supplier)`.
#[evento::subscription]
async fn on_charge_captured<E: evento::Executor>(
    ctx: &Context<'_, E>,
    event: Event<ChargeCaptured>,
) -> anyhow::Result<()> {
    let executor = executor(ctx)?;

    // A `ChargeCaptured` event carries only its own aggregate id, so the order
    // it pays for has to come from replaying the payment. Both lookups bail
    // rather than skip: the retry then covers a read that raced the write.
    let Some(payment) = load_payment(&executor, &event.aggregate_id).await? else {
        anyhow::bail!("captured payment {} cannot be loaded", event.aggregate_id);
    };
    let Some(order) = load_order(&executor, &payment.order_id).await? else {
        anyhow::bail!(
            "payment {} points at unknown order {}",
            payment.id,
            payment.order_id
        );
    };

    if order.status == OrderStatus::Cancelled {
        tracing::warn!(
            order_id = %order.id,
            payment_id = %payment.id,
            "charge captured for a cancelled order; not forwarding to suppliers"
        );
        return Ok(());
    }

    if order.status < OrderStatus::Paid {
        order
            .write()?
            .event(&OrderPaid {
                payment_id: payment.id.clone(),
            })
            .commit(&executor)
            .await?;
        tracing::info!(order_id = %order.id, payment_id = %payment.id, "order paid");
    }

    let registry = registry(ctx)?;
    for (supplier_id, lines) in by_supplier(&order.lines) {
        let supplier_order_id =
            forward_order(&executor, &registry, &order.id, &supplier_id, lines).await?;
        tracing::info!(
            order_id = %order.id,
            %supplier_id,
            %supplier_order_id,
            "order forwarded to supplier"
        );
    }

    Ok(())
}

/// Step 2' — the charge was refused. Nothing was captured, so there is nothing
/// to compensate; the order is simply cancelled.
#[evento::subscription]
async fn on_charge_failed<E: evento::Executor>(
    ctx: &Context<'_, E>,
    event: Event<ChargeFailed>,
) -> anyhow::Result<()> {
    let executor = executor(ctx)?;

    let Some(payment) = load_payment(&executor, &event.aggregate_id).await? else {
        anyhow::bail!("failed payment {} cannot be loaded", event.aggregate_id);
    };
    let Some(order) = load_order(&executor, &payment.order_id).await? else {
        anyhow::bail!(
            "payment {} points at unknown order {}",
            payment.id,
            payment.order_id
        );
    };

    // `Cancelled` ranks above `Paid`, so this one comparison also covers an
    // order that has already been cancelled by an earlier pass.
    if order.status > OrderStatus::Paid {
        tracing::warn!(
            order_id = %order.id,
            status = order.status.as_str(),
            "charge failed for an order that has moved on; leaving it alone"
        );
        return Ok(());
    }

    order
        .write()?
        .event(&OrderCancelled {
            reason: event.data.reason.clone(),
        })
        .commit(&executor)
        .await?;

    tracing::warn!(order_id = %order.id, reason = %event.data.reason, "order cancelled: charge failed");
    Ok(())
}

/// Step 3 — a supplier accepted, so there is a parcel to track.
///
/// The status only moves on the first confirmation: "forwarded" means the order
/// left our hands. A multi-supplier order therefore records only the first
/// supplier order id on [`OrderView`](crate::view::OrderView) — every supplier
/// order is still visible in the dropship admin, and splitting the status per
/// supplier needs a per-line fulfillment model this pass does not have.
#[evento::subscription]
async fn on_supplier_order_confirmed<E: evento::Executor>(
    ctx: &Context<'_, E>,
    event: Event<SupplierOrderConfirmed>,
) -> anyhow::Result<()> {
    let executor = executor(ctx)?;

    let Some(supplier_order) = load_supplier_order(&executor, &event.aggregate_id).await? else {
        anyhow::bail!(
            "confirmed supplier order {} cannot be loaded",
            event.aggregate_id
        );
    };

    create_shipment(
        &executor,
        &supplier_order.order_id,
        &supplier_order.supplier_id,
        &event.data.external_ref,
    )
    .await?;

    let Some(order) = load_order(&executor, &supplier_order.order_id).await? else {
        anyhow::bail!(
            "supplier order {} points at unknown order {}",
            supplier_order.id,
            supplier_order.order_id
        );
    };

    // `Cancelled` ranks above `Forwarded`, so the upper bound also excludes a
    // cancelled order. The lower bound keeps the ratchet honest: a confirmation
    // must not advance an order whose payment has not landed yet.
    if order.status >= OrderStatus::Paid && order.status < OrderStatus::Forwarded {
        order
            .write()?
            .event(&OrderForwardedToSupplier {
                supplier_order_id: supplier_order.id.clone(),
            })
            .commit(&executor)
            .await?;
        tracing::info!(
            order_id = %order.id,
            supplier_order_id = %supplier_order.id,
            "order forwarded"
        );
    }

    Ok(())
}

/// Step 3' — a supplier refused. **Compensation:** refund the captured charge,
/// then cancel.
///
/// The refund runs before the cancellation and is a no-op unless something was
/// actually captured, so a replay of this handler cannot refund twice and a
/// crash between the two leaves a refunded order that the next pass cancels.
#[evento::subscription]
async fn on_supplier_order_rejected<E: evento::Executor>(
    ctx: &Context<'_, E>,
    event: Event<SupplierOrderRejected>,
) -> anyhow::Result<()> {
    let executor = executor(ctx)?;

    let Some(supplier_order) = load_supplier_order(&executor, &event.aggregate_id).await? else {
        anyhow::bail!(
            "rejected supplier order {} cannot be loaded",
            event.aggregate_id
        );
    };
    let Some(order) = load_order(&executor, &supplier_order.order_id).await? else {
        anyhow::bail!(
            "supplier order {} points at unknown order {}",
            supplier_order.id,
            supplier_order.order_id
        );
    };

    if let Some(payment_id) = &order.payment_id {
        refund(&executor, &provider(ctx)?, payment_id).await?;
    }

    if matches!(
        order.status,
        OrderStatus::Cancelled | OrderStatus::Delivered
    ) {
        tracing::warn!(
            order_id = %order.id,
            status = order.status.as_str(),
            "supplier rejected an order that is already finished"
        );
        return Ok(());
    }

    order
        .write()?
        .event(&OrderCancelled {
            reason: event.data.reason.clone(),
        })
        .commit(&executor)
        .await?;

    tracing::warn!(
        order_id = %order.id,
        reason = %event.data.reason,
        "order cancelled: supplier rejected it"
    );
    Ok(())
}

/// Step 4 — the parcel is moving.
#[evento::subscription]
async fn on_shipment_dispatched<E: evento::Executor>(
    ctx: &Context<'_, E>,
    event: Event<ShipmentDispatched>,
) -> anyhow::Result<()> {
    let executor = executor(ctx)?;

    let Some(shipment) = load_shipment(&executor, &event.aggregate_id).await? else {
        anyhow::bail!(
            "dispatched shipment {} cannot be loaded",
            event.aggregate_id
        );
    };
    let Some(order) = load_order(&executor, &shipment.order_id).await? else {
        anyhow::bail!(
            "shipment {} points at unknown order {}",
            shipment.id,
            shipment.order_id
        );
    };

    // `Cancelled` ranks above `Shipped`, so a cancelled order is excluded here.
    if order.status < OrderStatus::Shipped {
        order
            .write()?
            .event(&OrderShipped {
                tracking_number: event.data.tracking_number.clone(),
            })
            .commit(&executor)
            .await?;
        tracing::info!(
            order_id = %order.id,
            tracking_number = %event.data.tracking_number,
            "order shipped"
        );
    }

    Ok(())
}

/// Step 5 — done.
#[evento::subscription]
async fn on_shipment_delivered<E: evento::Executor>(
    ctx: &Context<'_, E>,
    event: Event<ShipmentDelivered>,
) -> anyhow::Result<()> {
    let executor = executor(ctx)?;

    let Some(shipment) = load_shipment(&executor, &event.aggregate_id).await? else {
        anyhow::bail!("delivered shipment {} cannot be loaded", event.aggregate_id);
    };
    let Some(order) = load_order(&executor, &shipment.order_id).await? else {
        anyhow::bail!(
            "shipment {} points at unknown order {}",
            shipment.id,
            shipment.order_id
        );
    };

    if order.status < OrderStatus::Delivered {
        order
            .write()?
            .event(&OrderDelivered)
            .commit(&executor)
            .await?;
        tracing::info!(order_id = %order.id, "order delivered");
    }

    Ok(())
}
