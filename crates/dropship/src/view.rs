//! The write-side view of a supplier order, replayed from its events.
//!
//! Unlike [`crate::projections`] — the eventual-consistent SQL table that backs
//! the admin page — this is loaded on demand straight from the event store, so
//! a reader always sees every event committed so far. The fulfillment saga uses
//! it to map a supplier-order aggregate id (all a `SupplierOrderConfirmed`
//! event carries) back to the order and supplier it belongs to.

use evento::metadata::Event;
use evento::projection::Projection;
use timada_core::Executor;

use crate::aggregate::{
    SupplierOrder, SupplierOrderConfirmed, SupplierOrderPlaced, SupplierOrderRejected,
};

/// Where a supplier order stands. `Placed` means the supplier has not answered
/// yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, bitcode::Encode, bitcode::Decode)]
pub enum SupplierOrderStatus {
    #[default]
    Placed,
    Confirmed,
    Rejected,
}

#[evento::projection(bitcode::Encode, bitcode::Decode)]
#[derive(Debug, PartialEq, Eq)]
pub struct SupplierOrderView {
    pub id: String,
    pub order_id: String,
    pub supplier_id: String,
    /// The supplier's own reference — `Some` only once confirmed.
    pub external_ref: Option<String>,
    /// Why the supplier refused — `Some` only once rejected.
    pub reason: Option<String>,
    pub status: SupplierOrderStatus,
}

#[evento::handler]
async fn apply_placed(
    event: Event<SupplierOrderPlaced>,
    view: &mut SupplierOrderView,
) -> anyhow::Result<()> {
    view.id = event.aggregate_id.clone();
    view.order_id = event.data.order_id.clone();
    view.supplier_id = event.data.supplier_id.clone();
    view.status = SupplierOrderStatus::Placed;
    Ok(())
}

#[evento::handler]
async fn apply_confirmed(
    event: Event<SupplierOrderConfirmed>,
    view: &mut SupplierOrderView,
) -> anyhow::Result<()> {
    view.external_ref = Some(event.data.external_ref.clone());
    view.reason = None;
    view.status = SupplierOrderStatus::Confirmed;
    Ok(())
}

#[evento::handler]
async fn apply_rejected(
    event: Event<SupplierOrderRejected>,
    view: &mut SupplierOrderView,
) -> anyhow::Result<()> {
    view.reason = Some(event.data.reason.clone());
    view.status = SupplierOrderStatus::Rejected;
    Ok(())
}

/// Replay one supplier order. `None` means no such aggregate.
pub async fn load_supplier_order(
    executor: &Executor,
    supplier_order_id: &str,
) -> anyhow::Result<Option<SupplierOrderView>> {
    Projection::<_, SupplierOrderView>::new::<SupplierOrder>()
        .handler(apply_placed())
        .handler(apply_confirmed())
        .handler(apply_rejected())
        .strict()
        .load(supplier_order_id)
        .execute(executor)
        .await
}
