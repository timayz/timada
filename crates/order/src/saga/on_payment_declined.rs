use evento::{Executor, metadata::Event, subscription::Context};
use timada_payment::aggregator::PaymentDeclined;

use crate::value_object::FulfillmentStatus;

use super::{compensate, load_fulfillment};
use crate::payment_deadline::PAYMENT_TIMED_OUT;

/// `PaymentDeclined` → give the stock back and cancel the order. A payment
/// declined by [`crate::expire_unpaid_orders`] cancels it as timed out.
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
    let reason = if event.data.reason == PAYMENT_TIMED_OUT {
        PAYMENT_TIMED_OUT
    } else {
        "payment declined"
    };
    compensate(ctx.executor, &saga, reason).await
}
