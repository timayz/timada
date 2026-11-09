//! Where an order's parcels are ([`Shipment`](crate::aggregator::Shipment)):
//! the dashboard records what the carrier said, the storefront shows it.

use evento::{Executor, Projection, ProjectionAggregate, metadata::Event};

use crate::aggregator::{ParcelTracked, Shipment};
use crate::error::OrderError;
use crate::value_object::{Checkpoint, shipment_id};

/// One parcel of the order, its journey as last read.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ParcelState {
    pub number: String,
    /// In the carrier's order, oldest first.
    pub checkpoints: Vec<Checkpoint>,
    /// Unix seconds of the last read that found something new.
    pub updated_at: u64,
}

/// The parcels of one order ([`shipment_id`] of the order's id).
#[evento::projection(id = id, name = "timada-orders/ShipmentState")]
#[evento::snapshot(none)]
#[derive(Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ShipmentState {
    pub id: String,
    pub parcels: Vec<ParcelState>,
    /// Unix seconds of the last event.
    pub updated_at: u64,
}

pub fn shipment_projection<E: Executor>() -> Projection<E, ShipmentState> {
    Projection::new::<Shipment>()
        .handler(on_parcel_tracked())
        .strict()
}

#[evento::handler]
async fn on_parcel_tracked(
    event: Event<ParcelTracked>,
    state: &mut ShipmentState,
) -> anyhow::Result<()> {
    state.id = event.aggregate_id.to_owned();
    let at = event.timestamp;
    let data = event.data;
    match state
        .parcels
        .iter_mut()
        .find(|parcel| parcel.number == data.number)
    {
        Some(parcel) => {
            parcel.checkpoints = data.checkpoints;
            parcel.updated_at = at;
        }
        None => state.parcels.push(ParcelState {
            number: data.number,
            checkpoints: data.checkpoints,
            updated_at: at,
        }),
    }
    state.updated_at = at;
    Ok(())
}

#[evento::command]
impl<E: Executor> super::Command<E> {
    /// The order's parcels as last read; `None` while none was followed.
    pub async fn load_shipment(&self, order_id: &str) -> anyhow::Result<Option<ShipmentState>> {
        shipment_projection()
            .load(shipment_id(order_id))
            .tenant(&self.tenant)
            .execute(&self.executor)
            .await
    }

    /// Records the journey of the order's parcel `number` as the carrier
    /// gives it, whole. The same steps as last recorded write nothing:
    /// `Ok(false)`. Nothing is recorded of a parcel with no step yet, so the
    /// stream starts with the first news.
    pub async fn record_parcel_tracking(
        &self,
        actor: &str,
        order_id: &str,
        number: &str,
        checkpoints: Vec<Checkpoint>,
        routing_key: Option<String>,
    ) -> Result<bool, OrderError> {
        let number = number.trim();
        if number.is_empty() {
            return Err(OrderError::InvalidTracking);
        }
        let state = self.load_shipment(order_id).await?;
        let known = state
            .as_ref()
            .and_then(|state| state.parcels.iter().find(|parcel| parcel.number == number));
        match known {
            Some(parcel) if parcel.checkpoints == checkpoints => return Ok(false),
            None if checkpoints.is_empty() => return Ok(false),
            _ => {}
        }
        let mut write = match &state {
            Some(state) => state.write()?,
            None => {
                let mut write = evento::append(shipment_id(order_id));
                write.original_version(0);
                write
            }
        };
        write.routing_key_opt(routing_key);
        write.event(&ParcelTracked {
            number: number.to_owned(),
            checkpoints,
            tracked_by: actor.to_owned(),
        });
        write.tenant(&self.tenant).commit(&self.executor).await?;
        Ok(true)
    }
}
