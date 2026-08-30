//! Timada dropshipping.
//!
//! Suppliers are reached only through the [`Supplier`] trait, resolved at
//! runtime from a [`SupplierRegistry`] by the `supplier_id` snapshotted on
//! catalog products, cart lines and order lines. Forwarding an order is a write
//! against the [`SupplierOrder`] aggregate ([`forward_order`]); the fulfillment
//! saga in `timada-order` reacts to `SupplierOrderConfirmed` /
//! `SupplierOrderRejected` rather than calling suppliers itself.

mod aggregate;
mod commands;
mod migrations;
mod mock;
mod projections;
mod registry;
mod routes;
mod state;
mod supplier;
mod view;

pub use aggregate::{
    SupplierOrder, SupplierOrderConfirmed, SupplierOrderPlaced, SupplierOrderRejected,
};
pub use commands::{forward_order, supplier_order_id};
pub use migrations::migrations;
pub use mock::MockSupplier;
pub use projections::{
    ADMIN_SUBSCRIPTION, AdminSupplierOrderRow, admin_subscription, recent_supplier_orders,
    start_subscriptions,
};
pub use registry::{SupplierRegistry, SupplierRegistryBuilder};
pub use routes::admin_router;
pub use state::DropshipState;
pub use supplier::{
    Supplier, SupplierConfirmation, SupplierError, SupplierLine, SupplierOrderRequest,
    SupplierProduct, TrackingStatus,
};
pub use view::{SupplierOrderStatus, SupplierOrderView, load_supplier_order};
