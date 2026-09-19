use evento::{Executor, metadata::Event, subscription::Context};
use timada_inventory::{InventoryError, ReservationOutcome};

use crate::{
    aggregator::{FulfillmentStarted, OrderPlaced},
    query::load_order_details,
    value_object::{FulfillmentLine, FulfillmentStatus},
};

use super::{compensate, fulfillment_id, load_fulfillment, stock_location};

/// `OrderPlaced` → open the saga and ask inventory for every line. The
/// reservation outcomes come back as inventory events handled by the next
/// transitions.
#[evento::subscription]
pub(super) async fn start_fulfillment<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<OrderPlaced>,
) -> anyhow::Result<()> {
    let order_id = event.aggregate_id.to_owned();
    // A saga that exists and is still reserving is a previous delivery that
    // stopped half-way (a crash between two reservations): carry on, the
    // reservations are idempotent. Any later status means the work is done.
    let resuming = match load_fulfillment(ctx.executor, &order_id).await? {
        Some(saga) if saga.status != FulfillmentStatus::ReservingStock => return Ok(()),
        Some(_) => true,
        None => false,
    };

    // What is left to pay once `OrderDiscountApplied` (committed together
    // with this event) is taken off.
    let Some(order) = load_order_details(ctx.executor, &order_id).await? else {
        anyhow::bail!("order {order_id} placed but cannot be loaded");
    };
    let lines: Vec<FulfillmentLine> = event
        .data
        .lines
        .iter()
        .map(|l| FulfillmentLine {
            product_id: l.product_id.clone(),
            quantity: l.quantity,
        })
        .collect();

    let started = if resuming {
        Ok(String::new())
    } else {
        evento::append(fulfillment_id(&order_id))
            .event(&FulfillmentStarted {
                order_id: order_id.clone(),
                lines: lines.clone(),
                pickup_store_id: event.data.delivery.pickup_store_id.clone(),
                amount: order.total,
                payment_mode: event.data.payment_mode.clone(),
            })
            .commit(ctx.executor)
            .await
    };
    match started {
        Ok(_) => {}
        // Lost the race with a concurrent delivery of the same event.
        Err(evento::WriteError::InvalidOriginalVersion) => return Ok(()),
        Err(err) => return Err(err.into()),
    }
    tracing::info!(%order_id, resuming, "order fulfillment started");

    let location = stock_location(event.data.delivery.pickup_store_id.as_deref());
    let inventory = timada_inventory::Command(ctx.executor);
    for line in &lines {
        let stock_item = timada_inventory::stock_item_id(&line.product_id, &location);
        match inventory
            .reserve_stock(&stock_item, &order_id, line.quantity)
            .await
        {
            Ok(ReservationOutcome::Reserved | ReservationOutcome::Rejected { .. }) => {}
            // Nothing is stocked at that location at all: same as a rejection.
            Err(InventoryError::StockItemNotFound) => {
                let Some(saga) = load_fulfillment(ctx.executor, &order_id).await? else {
                    anyhow::bail!("fulfillment {order_id} vanished after start");
                };
                return compensate(ctx.executor, &saga, "out of stock").await;
            }
            Err(err) => return Err(err.into()),
        }
    }
    Ok(())
}
