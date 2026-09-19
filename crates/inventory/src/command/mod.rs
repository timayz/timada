mod cancel_back_in_stock_alert;
mod receive_stock;
mod register_stock_item;
mod release_stock;
mod request_back_in_stock_alert;
mod reserve_stock;
mod restock_return;
mod trigger_back_in_stock_alert;

use std::ops::Deref;

pub use register_stock_item::RegisterStockItem;
pub use request_back_in_stock_alert::RequestBackInStockAlert;
pub use trigger_back_in_stock_alert::trigger_alert;

use evento::{Executor, Projection, metadata::Event};

use crate::{
    aggregator::{
        BackInStockAlert, BackInStockAlertCancelled, BackInStockAlertRequested,
        BackInStockAlertTriggered, StockItem, StockItemRegistered, StockReceived,
        StockReservationRejected, StockReservationReleased, StockReserved, StockReturned,
    },
    error::InventoryError,
    value_object::StockLocation,
};

/// Deterministic stock item id: one per product and location.
pub fn stock_item_id(product_id: &str, location: &StockLocation) -> String {
    timada_core::id::derived(&[product_id, &location.key()], "stock")
}

/// Deterministic alert id: one per product and customer.
pub fn alert_id(product_id: &str, customer_id: &str) -> String {
    timada_core::id::derived(&[product_id, customer_id], "alert")
}

pub struct Command<'a, E: Executor>(pub &'a E);

impl<E: Executor> Deref for Command<'_, E> {
    type Target = E;

    fn deref(&self) -> &Self::Target {
        self.0
    }
}

impl<E: Executor> Command<'_, E> {
    pub async fn load_stock_item(
        &self,
        id: impl Into<String>,
    ) -> anyhow::Result<Option<StockItemState>> {
        load_stock_item(self.0, id).await
    }

    pub async fn load_alert(
        &self,
        id: impl Into<String>,
    ) -> anyhow::Result<Option<BackInStockAlertState>> {
        load_alert(self.0, id).await
    }

    async fn require_stock_item(
        &self,
        id: impl Into<String>,
    ) -> Result<StockItemState, InventoryError> {
        self.load_stock_item(id)
            .await?
            .ok_or(InventoryError::StockItemNotFound)
    }
}

pub(crate) async fn load_stock_item<E: Executor>(
    executor: &E,
    id: impl Into<String>,
) -> anyhow::Result<Option<StockItemState>> {
    stock_item_projection().load(id).execute(executor).await
}

pub(crate) async fn load_alert<E: Executor>(
    executor: &E,
    id: impl Into<String>,
) -> anyhow::Result<Option<BackInStockAlertState>> {
    alert_projection().load(id).execute(executor).await
}

/// Write-side state of a stock item: enough to decide reservations.
#[evento::projection(id = id)]
#[evento::snapshot(none)]
pub struct StockItemState {
    pub id: String,
    pub product_id: String,
    pub location: StockLocation,
    pub on_hand: u32,
    pub reserved: u32,
    /// Open reservations as `(order_id, quantity)`.
    pub reservations: Vec<(String, u32)>,
    /// Returns already put back into stock.
    pub restocked_returns: Vec<String>,
}

impl StockItemState {
    pub fn available(&self) -> u32 {
        self.on_hand.saturating_sub(self.reserved)
    }

    pub fn reservation_for(&self, order_id: &str) -> Option<u32> {
        self.reservations
            .iter()
            .find(|(id, _)| id == order_id)
            .map(|(_, quantity)| *quantity)
    }
}

// Strict + explicit skips: a non-strict projection only *reads* the events it
// handles, so the version it observes (and `write()` relies on) would go stale.
fn stock_item_projection<E: Executor>() -> Projection<E, StockItemState> {
    Projection::new::<StockItem>()
        .handler(on_stock_item_registered())
        .handler(on_stock_received())
        .handler(on_stock_returned())
        .handler(on_stock_reserved())
        .handler(on_stock_reservation_released())
        .skip::<StockReservationRejected>()
        .strict()
}

#[evento::handler]
async fn on_stock_item_registered(
    event: Event<StockItemRegistered>,
    row: &mut StockItemState,
) -> anyhow::Result<()> {
    row.id = event.aggregate_id.to_owned();
    row.product_id = event.data.product_id;
    row.location = event.data.location;
    Ok(())
}

#[evento::handler]
async fn on_stock_received(
    event: Event<StockReceived>,
    row: &mut StockItemState,
) -> anyhow::Result<()> {
    row.on_hand = row.on_hand.saturating_add(event.data.quantity);
    Ok(())
}

#[evento::handler]
async fn on_stock_returned(
    event: Event<StockReturned>,
    row: &mut StockItemState,
) -> anyhow::Result<()> {
    row.on_hand = row.on_hand.saturating_add(event.data.quantity);
    row.restocked_returns.push(event.data.return_id);
    Ok(())
}

#[evento::handler]
async fn on_stock_reserved(
    event: Event<StockReserved>,
    row: &mut StockItemState,
) -> anyhow::Result<()> {
    row.reserved = row.reserved.saturating_add(event.data.quantity);
    row.reservations
        .push((event.data.order_id, event.data.quantity));
    Ok(())
}

#[evento::handler]
async fn on_stock_reservation_released(
    event: Event<StockReservationReleased>,
    row: &mut StockItemState,
) -> anyhow::Result<()> {
    row.reserved = row.reserved.saturating_sub(event.data.quantity);
    row.reservations
        .retain(|(order_id, _)| *order_id != event.data.order_id);
    Ok(())
}

/// Write-side state of a back-in-stock alert.
#[evento::projection(id = id)]
#[evento::snapshot(none)]
pub struct BackInStockAlertState {
    pub id: String,
    pub product_id: String,
    pub customer_id: String,
    pub email: String,
    pub triggered: bool,
    pub cancelled: bool,
}

impl BackInStockAlertState {
    /// Still waiting for the product to come back.
    pub fn is_pending(&self) -> bool {
        !self.triggered && !self.cancelled
    }
}

fn alert_projection<E: Executor>() -> Projection<E, BackInStockAlertState> {
    Projection::new::<BackInStockAlert>()
        .handler(on_alert_requested())
        .handler(on_alert_triggered())
        .handler(on_alert_cancelled())
        .strict()
}

#[evento::handler]
async fn on_alert_requested(
    event: Event<BackInStockAlertRequested>,
    row: &mut BackInStockAlertState,
) -> anyhow::Result<()> {
    row.id = event.aggregate_id.to_owned();
    row.product_id = event.data.product_id;
    row.customer_id = event.data.customer_id;
    row.email = event.data.email;
    // A request after the alert fired, or was cancelled, arms it again.
    row.triggered = false;
    row.cancelled = false;
    Ok(())
}

#[evento::handler]
async fn on_alert_triggered(
    _event: Event<BackInStockAlertTriggered>,
    row: &mut BackInStockAlertState,
) -> anyhow::Result<()> {
    row.triggered = true;
    Ok(())
}

#[evento::handler]
async fn on_alert_cancelled(
    _event: Event<BackInStockAlertCancelled>,
    row: &mut BackInStockAlertState,
) -> anyhow::Result<()> {
    row.cancelled = true;
    Ok(())
}
