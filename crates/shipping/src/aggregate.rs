//! The `Shipment` aggregate: one supplier's parcel for one order.
//!
//! The consistency boundary is narrow on purpose — a shipment only records what
//! the supplier told us about a parcel. It holds no order totals and no
//! customer data; the customer-facing `Order` reacts to these events through the
//! fulfillment saga.
//!
//! The state machine is deliberately permissive: `ShipmentDelivered` may arrive
//! without a preceding `ShipmentDispatched`, because some suppliers only report
//! a parcel once it has landed.

#[evento::aggregate]
pub enum Shipment {
    /// A supplier confirmed a forwarded order, so there is a parcel to track.
    /// `external_ref` is the supplier's own reference, the key tracking is
    /// polled with.
    ShipmentCreated {
        order_id: String,
        supplier_id: String,
        external_ref: String,
    },
    /// The supplier handed the parcel to a carrier.
    ShipmentDispatched {
        tracking_number: String,
        carrier: String,
    },
    /// The parcel reached the customer.
    ShipmentDelivered,
}
