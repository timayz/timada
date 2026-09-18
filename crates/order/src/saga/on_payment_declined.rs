use evento::{Executor, metadata::Event, subscription::Context};
use timada_payment::aggregator::PaymentDeclined;

use crate::value_object::FulfillmentStatus;

use super::{compensate, load_fulfillment};

/// `PaymentDeclined` → give the stock back and cancel the order.
#[evento::subscription]
pub(super) async fn compensate_declined_payment<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<PaymentDeclined>,
) -> anyhow::Result<()> {
    let Some(payment) = timada_payment::load_payment(ctx.executor, &event.aggregate_id).await?
    else {
        anyhow::bail!(
            "payment {} declined but cannot be loaded",
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
    compensate(ctx.executor, &saga, "payment declined").await
}
