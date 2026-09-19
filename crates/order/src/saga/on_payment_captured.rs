use evento::{Executor, ProjectionAggregate, metadata::Event, subscription::Context};
use timada_payment::aggregator::PaymentCaptured;

use crate::{
    aggregator::{PaymentCaptured as SagaPaymentCaptured, ShipmentRequested},
    command::Command,
    value_object::FulfillmentStatus,
};

use super::{create_shipment, load_fulfillment};

/// `PaymentCaptured` → mark the order paid and hand it to shipping. Shipment
/// creation is idempotent (id derived from the order) and `mark_paid` accepts
/// a repeat for the same payment, so a redelivery converges.
#[evento::subscription]
pub(super) async fn request_shipment<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<PaymentCaptured>,
) -> anyhow::Result<()> {
    let Some(payment) = timada_payment::load_payment(ctx.executor, &event.aggregate_id).await?
    else {
        anyhow::bail!(
            "payment {} captured but cannot be loaded",
            event.aggregate_id
        );
    };
    let Some(saga) = load_fulfillment(ctx.executor, &payment.order_id).await? else {
        return Ok(());
    };
    if saga.status != FulfillmentStatus::AwaitingPayment
        || saga.payment_id.as_deref() != Some(&payment.id)
    {
        return Ok(());
    }

    let shipment_id = create_shipment(ctx.executor, &saga).await?;
    Command(ctx.executor)
        .mark_paid(&saga.order_id, &payment.id)
        .await?;

    saga.write()?
        .event(&SagaPaymentCaptured)
        .event(&ShipmentRequested { shipment_id })
        .commit(ctx.executor)
        .await?;
    tracing::info!(order_id = %saga.order_id, "payment captured, shipment requested");
    Ok(())
}
