use evento::{Executor, metadata::Event, subscription::Context};

use crate::{aggregator::OrderCancelled, value_object::FulfillmentStatus};

use super::{compensate, load_fulfillment};

/// `OrderCancelled` from outside the saga (an operator, the customer) → the
/// stock goes back and a captured payment is refunded. A cancellation the
/// saga made itself finds it already compensated.
#[evento::subscription]
pub(super) async fn compensate_cancelled_order<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<OrderCancelled>,
) -> anyhow::Result<()> {
    let Some(saga) = load_fulfillment(ctx.executor, &event.aggregate_id).await? else {
        return Ok(());
    };
    if matches!(
        saga.status,
        FulfillmentStatus::Compensated | FulfillmentStatus::Completed
    ) {
        return Ok(());
    }
    compensate(ctx.executor, &saga, &event.data.reason).await
}
