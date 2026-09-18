use evento::{Executor, metadata::Event, subscription::Context};
use timada_inventory::aggregator::StockReservationRejected;

use crate::value_object::FulfillmentStatus;

use super::{compensate, load_fulfillment};

/// `StockReservationRejected` → release what was already reserved and cancel
/// the order ("rupture").
#[evento::subscription]
pub(super) async fn compensate_out_of_stock<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<StockReservationRejected>,
) -> anyhow::Result<()> {
    let Some(saga) = load_fulfillment(ctx.executor, &event.data.order_id).await? else {
        return Ok(());
    };
    if saga.status != FulfillmentStatus::ReservingStock
        || saga.line_for_stock_item(&event.aggregate_id).is_none()
    {
        return Ok(());
    }
    compensate(ctx.executor, &saga, "out of stock").await
}
