use evento::{Executor, ProjectionAggregate};

use crate::{
    aggregator::{StockReservationRejected, StockReserved},
    error::InventoryError,
    value_object::ReservationOutcome,
};

impl<E: Executor> super::Command<E> {
    /// Puts units aside for an order. Idempotent per order: a repeated call
    /// for an order that already holds a reservation reports `Reserved`
    /// without writing. Both outcomes are recorded so the fulfillment saga can
    /// react to them.
    pub async fn reserve_stock(
        &self,
        id: impl Into<String>,
        order_id: impl Into<String>,
        quantity: u32,
    ) -> Result<ReservationOutcome, InventoryError> {
        if quantity == 0 {
            return Err(InventoryError::InvalidQuantity);
        }
        let order_id = order_id.into();
        if order_id.trim().is_empty() {
            return Err(InventoryError::Required("order_id"));
        }
        let item = self.require_stock_item(id).await?;

        if item.reservation_for(&order_id).is_some() {
            return Ok(ReservationOutcome::Reserved);
        }

        let available = item.available();
        if available >= quantity {
            item.write()?
                .event(&StockReserved { order_id, quantity })
                .commit(&self.0)
                .await?;
            return Ok(ReservationOutcome::Reserved);
        }

        item.write()?
            .event(&StockReservationRejected {
                order_id,
                requested: quantity,
                available,
            })
            .commit(&self.0)
            .await?;
        Ok(ReservationOutcome::Rejected { available })
    }
}
