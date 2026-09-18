use evento::{Executor, ProjectionAggregate, metadata::Event, subscription::Context};
use timada_shipping::aggregator::ShipmentDispatched;

use crate::{aggregator::FulfillmentCompleted, command::Command, value_object::FulfillmentStatus};

use super::load_fulfillment;

/// `ShipmentDispatched` → the order is shipped; the saga is done.
#[evento::subscription]
pub(super) async fn complete_fulfillment<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<ShipmentDispatched>,
) -> anyhow::Result<()> {
    let Some(shipment) = timada_shipping::load_shipment(ctx.executor, &event.aggregate_id).await?
    else {
        anyhow::bail!(
            "shipment {} dispatched but cannot be loaded",
            event.aggregate_id
        );
    };
    let Some(saga) = load_fulfillment(ctx.executor, &shipment.order_id).await? else {
        return Ok(());
    };
    if saga.status != FulfillmentStatus::AwaitingShipment
        || saga.shipment_id.as_deref() != Some(&shipment.id)
    {
        return Ok(());
    }

    Command(ctx.executor)
        .mark_shipped(
            &saga.order_id,
            &shipment.id,
            event.data.carrier,
            event.data.tracking_number,
        )
        .await?;
    saga.write()?
        .event(&FulfillmentCompleted)
        .commit(ctx.executor)
        .await?;
    tracing::info!(order_id = %saga.order_id, "order fulfillment completed");
    Ok(())
}
