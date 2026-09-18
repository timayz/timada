use evento::Executor;
use timada_core::Address;

use crate::{
    aggregator::ShipmentCreated,
    error::ShippingError,
    value_object::{DeliveryMethod, ShipmentLine},
};

use super::shipment_id;

#[derive(Debug, Clone)]
pub struct CreateShipment {
    pub order_id: String,
    pub method: DeliveryMethod,
    pub destination: Address,
    pub lines: Vec<ShipmentLine>,
}

#[evento::command]
impl<E: Executor> super::Command<E> {
    /// Prepares the shipment for an order. The id is derived from the order
    /// id, so a retry (e.g. from the fulfillment saga) finds the shipment
    /// already created and returns its id instead of failing.
    pub async fn create_shipment(
        &self,
        cmd: CreateShipment,
        routing_key: Option<String>,
    ) -> Result<String, ShippingError> {
        if cmd.order_id.trim().is_empty() {
            return Err(ShippingError::Required("order_id"));
        }
        if cmd.lines.is_empty() {
            return Err(ShippingError::NoLines);
        }
        cmd.destination.validate()?;

        let id = shipment_id(&cmd.order_id);
        let result = evento::append(&id)
            .routing_key_opt(routing_key)
            .event(&ShipmentCreated {
                order_id: cmd.order_id.clone(),
                method: cmd.method,
                destination: cmd.destination,
                lines: cmd.lines,
            })
            .commit(&self.0)
            .await;

        match result {
            Ok(id) => {
                tracing::info!(shipment_id = %id, order_id = %cmd.order_id, "shipment created");
                Ok(id)
            }
            Err(evento::WriteError::InvalidOriginalVersion) => Ok(id),
            Err(err) => Err(err.into()),
        }
    }
}
