//! What the product page shows next to "Livraison à domicile" / "Retrait en
//! boutique": available units and an in/out of stock flag per location.

use evento::{Executor, metadata::Event, projection::Projection};

use crate::{
    aggregator::{
        StockItem, StockItemRegistered, StockLevelSynced, StockReceived, StockReservationRejected,
        StockReservationReleased, StockReserved, StockReturned,
    },
    value_object::{Availability, StockLocation},
};

#[evento::projection(bitcode::Encode, bitcode::Decode)]
#[derive(Debug, PartialEq)]
pub struct StockAvailabilityView {
    pub id: String,
    pub product_id: String,
    pub location: StockLocation,
    pub on_hand: u32,
    pub reserved: u32,
    pub available: u32,
    pub status: Availability,
}

impl StockAvailabilityView {
    fn refresh(&mut self) {
        self.available = self.on_hand.saturating_sub(self.reserved);
        self.status = if self.available > 0 {
            Availability::InStock
        } else {
            Availability::OutOfStock
        };
    }
}

pub fn create_projection<E: Executor>() -> Projection<E, StockAvailabilityView> {
    Projection::new::<StockItem>()
        .handler(on_stock_item_registered())
        .handler(on_stock_received())
        .handler(on_stock_returned())
        .handler(on_stock_level_synced())
        .handler(on_stock_reserved())
        .handler(on_stock_reservation_released())
        .skip::<StockReservationRejected>()
        .strict()
}

pub async fn load<E: Executor>(
    executor: &E,
    id: impl Into<String>,
) -> anyhow::Result<Option<StockAvailabilityView>> {
    create_projection().load(id).execute(executor).await
}

#[evento::handler]
async fn on_stock_item_registered(
    event: Event<StockItemRegistered>,
    row: &mut StockAvailabilityView,
) -> anyhow::Result<()> {
    row.id = event.aggregate_id.to_owned();
    row.product_id = event.data.product_id;
    row.location = event.data.location;
    row.refresh();
    Ok(())
}

#[evento::handler]
async fn on_stock_received(
    event: Event<StockReceived>,
    row: &mut StockAvailabilityView,
) -> anyhow::Result<()> {
    row.on_hand = row.on_hand.saturating_add(event.data.quantity);
    row.refresh();
    Ok(())
}

#[evento::handler]
async fn on_stock_returned(
    event: Event<StockReturned>,
    row: &mut StockAvailabilityView,
) -> anyhow::Result<()> {
    row.on_hand = row.on_hand.saturating_add(event.data.quantity);
    row.refresh();
    Ok(())
}

/// A supplier's feed, or a stock-take, saying what can still be sold. What
/// is already put aside for orders is not theirs to move, so the level goes
/// on top of `reserved` rather than replacing `on_hand` outright.
#[evento::handler]
async fn on_stock_level_synced(
    event: Event<StockLevelSynced>,
    row: &mut StockAvailabilityView,
) -> anyhow::Result<()> {
    row.on_hand = row.reserved.saturating_add(event.data.available);
    row.refresh();
    Ok(())
}

#[evento::handler]
async fn on_stock_reserved(
    event: Event<StockReserved>,
    row: &mut StockAvailabilityView,
) -> anyhow::Result<()> {
    row.reserved = row.reserved.saturating_add(event.data.quantity);
    row.refresh();
    Ok(())
}

#[evento::handler]
async fn on_stock_reservation_released(
    event: Event<StockReservationReleased>,
    row: &mut StockAvailabilityView,
) -> anyhow::Result<()> {
    row.reserved = row.reserved.saturating_sub(event.data.quantity);
    row.refresh();
    Ok(())
}
