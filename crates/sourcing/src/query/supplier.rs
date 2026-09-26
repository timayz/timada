//! A supplier as the back office shows it. The one snapshotted view of the
//! `Supplier` aggregate — evento keys a snapshot by aggregate, not by view,
//! so a second one here would overwrite it.

use evento::{Executor, metadata::Event, projection::Projection};

use crate::aggregator::{
    Supplier, SupplierRegistered, SupplierRenamed, SupplierResumed, SupplierSuspended,
};

#[evento::projection(bitcode::Encode, bitcode::Decode)]
#[derive(Debug, PartialEq)]
pub struct SupplierView {
    pub id: String,
    pub slug: String,
    pub name: String,
    /// The adapter that talks to it: `"manual"`, `"aliexpress"`, …
    pub connector: String,
    /// What it quotes its costs in.
    pub currency: String,
    pub suspended: bool,
    pub suspended_reason: String,
    /// Unix seconds of `SupplierRegistered`.
    pub registered_at: u64,
}

pub fn create_projection<E: Executor>() -> Projection<E, SupplierView> {
    Projection::new::<Supplier>()
        .handler(on_supplier_registered())
        .handler(on_supplier_renamed())
        .handler(on_supplier_suspended())
        .handler(on_supplier_resumed())
        .strict()
}

pub async fn load<E: Executor>(
    executor: &E,
    id: impl Into<String>,
) -> anyhow::Result<Option<SupplierView>> {
    create_projection().load(id).execute(executor).await
}

#[evento::handler]
async fn on_supplier_registered(
    event: Event<SupplierRegistered>,
    row: &mut SupplierView,
) -> anyhow::Result<()> {
    row.id = event.aggregate_id.to_owned();
    row.registered_at = event.timestamp;
    row.slug = event.data.slug;
    row.name = event.data.name;
    row.connector = event.data.connector;
    row.currency = event.data.currency;
    Ok(())
}

#[evento::handler]
async fn on_supplier_renamed(
    event: Event<SupplierRenamed>,
    row: &mut SupplierView,
) -> anyhow::Result<()> {
    row.name = event.data.name;
    Ok(())
}

#[evento::handler]
async fn on_supplier_suspended(
    event: Event<SupplierSuspended>,
    row: &mut SupplierView,
) -> anyhow::Result<()> {
    row.suspended = true;
    row.suspended_reason = event.data.reason;
    Ok(())
}

#[evento::handler]
async fn on_supplier_resumed(
    _event: Event<SupplierResumed>,
    row: &mut SupplierView,
) -> anyhow::Result<()> {
    row.suspended = false;
    row.suspended_reason = String::new();
    Ok(())
}
