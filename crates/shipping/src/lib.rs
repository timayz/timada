//! Timada shipping.
//!
//! A [`Shipment`] tracks one supplier's parcel for one order. Timada never
//! learns of a dispatch on its own: [`refresh_tracking`] polls the supplier
//! through `timada-dropship`'s registry and turns whatever comes back into at
//! most one new event. The admin "Refresh tracking" button is what drives it in
//! the demo; a scheduled poller would call the same command.
//!
//! The order-fulfillment saga in `timada-order` reacts to `ShipmentDispatched`
//! / `ShipmentDelivered` and uses [`load_shipment`] to map a shipment aggregate
//! id back to the order it belongs to.

mod aggregate;
mod commands;
mod migrations;
mod projections;
mod routes;
mod state;
mod view;

pub use aggregate::{Shipment, ShipmentCreated, ShipmentDelivered, ShipmentDispatched};
pub use commands::{create_shipment, refresh_tracking, shipment_id};
pub use migrations::migrations;
pub use projections::{
    ADMIN_SUBSCRIPTION, AdminShipmentRow, admin_subscription, recent_shipments, start_subscriptions,
};
pub use routes::admin_router;
pub use state::ShippingState;
pub use view::{ShipmentView, load_shipment};
