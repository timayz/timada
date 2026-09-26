//! Where one product is bought, as the back office shows it.

use evento::{Executor, metadata::Event, projection::Projection};

use crate::aggregator::{
    ProductSourced, SourcePriceLocked, SourcePriceUnlocked, SourcedProduct, SourcingStopped,
};

#[evento::projection(bitcode::Encode, bitcode::Decode)]
#[derive(Debug, PartialEq)]
pub struct SourcedProductView {
    pub id: String,
    pub product_id: String,
    pub supplier_id: String,
    pub external_item_id: String,
    pub external_sku: Option<String>,
    /// Still bought from somebody.
    pub active: bool,
    /// The operator's price stands: the sync neither moves it nor asks.
    pub locked: bool,
    pub locked_reason: String,
    /// Unix seconds of the latest `ProductSourced`.
    pub sourced_at: u64,
}

pub fn create_projection<E: Executor>() -> Projection<E, SourcedProductView> {
    Projection::new::<SourcedProduct>()
        .handler(on_product_sourced())
        .handler(on_sourcing_stopped())
        .handler(on_source_price_locked())
        .handler(on_source_price_unlocked())
        .strict()
}

pub async fn load<E: Executor>(
    executor: &E,
    id: impl Into<String>,
) -> anyhow::Result<Option<SourcedProductView>> {
    create_projection().load(id).execute(executor).await
}

#[evento::handler]
async fn on_product_sourced(
    event: Event<ProductSourced>,
    row: &mut SourcedProductView,
) -> anyhow::Result<()> {
    row.id = event.aggregate_id.to_owned();
    row.sourced_at = event.timestamp;
    row.product_id = event.data.product_id;
    row.supplier_id = event.data.supplier_id;
    row.external_item_id = event.data.external_item_id;
    row.external_sku = event.data.external_sku;
    row.active = true;
    Ok(())
}

#[evento::handler]
async fn on_sourcing_stopped(
    _event: Event<SourcingStopped>,
    row: &mut SourcedProductView,
) -> anyhow::Result<()> {
    row.active = false;
    Ok(())
}

#[evento::handler]
async fn on_source_price_locked(
    event: Event<SourcePriceLocked>,
    row: &mut SourcedProductView,
) -> anyhow::Result<()> {
    row.locked = true;
    row.locked_reason = event.data.reason;
    Ok(())
}

#[evento::handler]
async fn on_source_price_unlocked(
    _event: Event<SourcePriceUnlocked>,
    row: &mut SourcedProductView,
) -> anyhow::Result<()> {
    row.locked = false;
    row.locked_reason = String::new();
    Ok(())
}
