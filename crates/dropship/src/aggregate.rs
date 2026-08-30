//! The `SupplierOrder` aggregate: one order's worth of lines handed to one
//! supplier.
//!
//! The consistency boundary is deliberately narrow — it records that we asked a
//! supplier to fulfill part of an order and what the supplier answered. The
//! customer-facing `Order` reacts to these events through the fulfillment saga.

use crate::supplier::SupplierLine;

#[evento::aggregate]
pub enum SupplierOrder {
    /// We decided to forward these lines; the supplier has not answered yet.
    SupplierOrderPlaced {
        order_id: String,
        supplier_id: String,
        lines: Vec<SupplierLine>,
    },
    /// The supplier accepted and gave us its own reference.
    SupplierOrderConfirmed { external_ref: String },
    /// The supplier refused, or we could not reach it at all.
    SupplierOrderRejected { reason: String },
}
