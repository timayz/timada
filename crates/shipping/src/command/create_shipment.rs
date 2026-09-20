use evento::Executor;
use timada_core::Address;

use crate::{
    aggregator::{ShipmentCreated, ShipmentReplacesReturn},
    error::ShippingError,
    value_object::{DeliveryMethod, ShipmentLine},
};

use super::{replacement_shipment_id, shipment_id};

#[derive(Debug, Clone)]
pub struct CreateShipment {
    pub order_id: String,
    pub method: DeliveryMethod,
    pub destination: Address,
    pub lines: Vec<ShipmentLine>,
}

#[evento::command]
impl<E: Executor> super::Command<'_, E> {
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
            .commit(self.0)
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

impl<E: Executor> super::Command<'_, E> {
    /// Prepares a second parcel for an order: the replacement of what a
    /// return brought back. `reference` is the return's id — the shipment's
    /// id derives from it, so a retry finds it already created. It carries
    /// the order's id like the first parcel, and says what it replaces.
    pub async fn create_replacement_shipment(
        &self,
        cmd: CreateShipment,
        reference: &str,
    ) -> Result<String, ShippingError> {
        if cmd.order_id.trim().is_empty() {
            return Err(ShippingError::Required("order_id"));
        }
        if reference.trim().is_empty() {
            return Err(ShippingError::Required("reference"));
        }
        if cmd.lines.is_empty() {
            return Err(ShippingError::NoLines);
        }
        cmd.destination.validate()?;

        let id = replacement_shipment_id(reference);
        let result = evento::append(&id)
            .event(&ShipmentCreated {
                order_id: cmd.order_id.clone(),
                method: cmd.method,
                destination: cmd.destination,
                lines: cmd.lines,
            })
            .event(&ShipmentReplacesReturn {
                reference: reference.to_owned(),
            })
            .commit(self.0)
            .await;
        match result {
            Ok(id) => {
                tracing::info!(shipment_id = %id, order_id = %cmd.order_id, %reference, "replacement shipment created");
                Ok(id)
            }
            Err(evento::WriteError::InvalidOriginalVersion) => Ok(id),
            Err(err) => Err(err.into()),
        }
    }
}
