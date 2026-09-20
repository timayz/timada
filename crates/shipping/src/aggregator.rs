use timada_core::Address;

use crate::value_object::{DeliveryMethod, ShipmentLine};

// The explicit name pins the on-disk identity: renaming the crate or the enum
// must never orphan stored events.
#[evento::aggregate(name = "timada-shipping/Shipment")]
pub enum Shipment {
    /// A shipment was prepared for a paid order.
    ShipmentCreated {
        order_id: String,
        method: DeliveryMethod,
        destination: Address,
        lines: Vec<ShipmentLine>,
    },

    /// The carrier picked the parcel up and issued a tracking number.
    ShipmentDispatched {
        carrier: String,
        tracking_number: String,
    },

    /// The parcel reached its destination.
    ShipmentDelivered,

    /// The order was cancelled before the parcel left: nothing will ship.
    ShipmentCancelled { reason: String },

    /// The shipment is not the order's own parcel but a replacement sent for
    /// a return: `reference` is the return's id. Committed together with
    /// `ShipmentCreated`, never on its own.
    ShipmentReplacesReturn { reference: String },
}
