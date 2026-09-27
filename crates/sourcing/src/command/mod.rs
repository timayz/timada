mod apply_offer;
mod edit_supplier;
mod lock_source_price;
mod register_supplier;
mod settle_review;
mod source_product;
mod supplier_order;

pub use apply_offer::Applied;
pub use register_supplier::RegisterSupplier;
pub use source_product::SourceProduct;
pub use supplier_order::DraftSupplierOrder;

use evento::{Executor, Projection, metadata::Event};
use sqlx::SqlitePool;

use crate::{
    aggregator::{
        ProductSourced, SourcePriceLocked, SourcePriceUnlocked, SourcedProduct, SourcingStopped,
        Supplier, SupplierOrder, SupplierOrderCancelled, SupplierOrderDrafted, SupplierOrderPlaced,
        SupplierOrderRecordedByHand, SupplierOrderRefused, SupplierOrderShipped,
        SupplierRegistered, SupplierRenamed, SupplierResumed, SupplierSuspended,
    },
    error::SourcingError,
    value_object::{SupplierOrderLine, SupplierOrderStatus},
};

/// Deterministic supplier id, from the slug it is known by — which is for
/// ever, the way a category's is.
pub fn supplier_id(slug: &str) -> String {
    timada_core::id::derived(&[slug], "supplier")
}

/// Deterministic sourcing id: one stream per product. That a product is
/// bought from one supplier at a time is therefore structural, not a rule
/// enforced by an index somewhere.
pub fn sourced_product_id(product_id: &str) -> String {
    timada_core::id::derived(&[product_id], "sourced-product")
}

/// Deterministic purchase id: one per customer order and supplier. An order
/// split across two suppliers therefore has two purchases by construction.
pub fn supplier_order_id(order_id: &str, supplier_id: &str) -> String {
    timada_core::id::derived(&[order_id, supplier_id], "supplier-order")
}

/// Sourcing needs the pool next to the executor: the pricing rule and the
/// suppliers' last word are configuration and operational data, kept in SQL.
pub struct Command<'a, E: Executor> {
    pub executor: &'a E,
    pub db: SqlitePool,
}

impl<'a, E: Executor> Command<'a, E> {
    pub fn new(executor: &'a E, db: SqlitePool) -> Self {
        Self { executor, db }
    }

    pub async fn load_supplier(
        &self,
        id: impl Into<String>,
    ) -> anyhow::Result<Option<SupplierState>> {
        supplier_projection().load(id).execute(self.executor).await
    }

    pub async fn load_sourced(
        &self,
        id: impl Into<String>,
    ) -> anyhow::Result<Option<SourcedProductState>> {
        sourced_projection().load(id).execute(self.executor).await
    }

    /// The sourcing of a product, by the product's own id.
    pub async fn load_sourced_product(
        &self,
        product_id: &str,
    ) -> anyhow::Result<Option<SourcedProductState>> {
        self.load_sourced(sourced_product_id(product_id)).await
    }

    pub(crate) async fn require_supplier(
        &self,
        id: impl Into<String>,
    ) -> Result<SupplierState, SourcingError> {
        self.load_supplier(id)
            .await?
            .ok_or(SourcingError::SupplierNotFound)
    }

    pub async fn load_purchase(
        &self,
        id: impl Into<String>,
    ) -> anyhow::Result<Option<SupplierOrderState>> {
        purchase_projection().load(id).execute(self.executor).await
    }

    pub(crate) async fn require_purchase(
        &self,
        id: impl Into<String>,
    ) -> Result<SupplierOrderState, SourcingError> {
        self.load_purchase(id)
            .await?
            .ok_or(SourcingError::SupplierOrderNotFound)
    }

    pub(crate) async fn require_sourced(
        &self,
        id: impl Into<String>,
    ) -> Result<SourcedProductState, SourcingError> {
        self.load_sourced(id)
            .await?
            .filter(|sourced| sourced.active)
            .ok_or(SourcingError::NotSourced)
    }
}

/// Write-side state of a supplier: just enough to guard the commands.
#[evento::projection(id = id)]
#[evento::snapshot(none)]
pub struct SupplierState {
    pub id: String,
    pub slug: String,
    pub name: String,
    pub connector: String,
    pub currency: String,
    pub suspended: bool,
}

// Strict + explicit skips: a non-strict projection only *reads* the events it
// handles, so the version it observes (and `write()` relies on) would go stale.
fn supplier_projection<E: Executor>() -> Projection<E, SupplierState> {
    Projection::new::<Supplier>()
        .handler(on_supplier_registered())
        .handler(on_supplier_renamed())
        .handler(on_supplier_suspended())
        .handler(on_supplier_resumed())
        .strict()
}

#[evento::handler]
async fn on_supplier_registered(
    event: Event<SupplierRegistered>,
    row: &mut SupplierState,
) -> anyhow::Result<()> {
    row.id = event.aggregate_id.to_owned();
    row.slug = event.data.slug;
    row.name = event.data.name;
    row.connector = event.data.connector;
    row.currency = event.data.currency;
    Ok(())
}

#[evento::handler]
async fn on_supplier_renamed(
    event: Event<SupplierRenamed>,
    row: &mut SupplierState,
) -> anyhow::Result<()> {
    row.name = event.data.name;
    Ok(())
}

#[evento::handler]
async fn on_supplier_suspended(
    _event: Event<SupplierSuspended>,
    row: &mut SupplierState,
) -> anyhow::Result<()> {
    row.suspended = true;
    Ok(())
}

#[evento::handler]
async fn on_supplier_resumed(
    _event: Event<SupplierResumed>,
    row: &mut SupplierState,
) -> anyhow::Result<()> {
    row.suspended = false;
    Ok(())
}

/// Write-side state of a sourced product.
#[evento::projection(id = id)]
#[evento::snapshot(none)]
pub struct SourcedProductState {
    pub id: String,
    pub product_id: String,
    pub supplier_id: String,
    pub external_item_id: String,
    pub external_sku: Option<String>,
    /// Still bought from somebody: `stop_sourcing` clears it, sourcing it
    /// again sets it back.
    pub active: bool,
    pub locked: bool,
    pub locked_reason: String,
}

fn sourced_projection<E: Executor>() -> Projection<E, SourcedProductState> {
    Projection::new::<SourcedProduct>()
        .handler(on_product_sourced())
        .handler(on_sourcing_stopped())
        .handler(on_source_price_locked())
        .handler(on_source_price_unlocked())
        .strict()
}

#[evento::handler]
async fn on_product_sourced(
    event: Event<ProductSourced>,
    row: &mut SourcedProductState,
) -> anyhow::Result<()> {
    row.id = event.aggregate_id.to_owned();
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
    row: &mut SourcedProductState,
) -> anyhow::Result<()> {
    row.active = false;
    Ok(())
}

#[evento::handler]
async fn on_source_price_locked(
    event: Event<SourcePriceLocked>,
    row: &mut SourcedProductState,
) -> anyhow::Result<()> {
    row.locked = true;
    row.locked_reason = event.data.reason;
    Ok(())
}

#[evento::handler]
async fn on_source_price_unlocked(
    _event: Event<SourcePriceUnlocked>,
    row: &mut SourcedProductState,
) -> anyhow::Result<()> {
    row.locked = false;
    row.locked_reason = String::new();
    Ok(())
}

/// Write-side state of a purchase: enough to guard the commands.
#[evento::projection(id = id)]
#[evento::snapshot(none)]
pub struct SupplierOrderState {
    pub id: String,
    pub order_id: String,
    pub supplier_id: String,
    pub lines: Vec<SupplierOrderLine>,
    /// Where the supplier sends it: the customer's own address.
    pub ship_to: timada_core::Address,
    /// What it was expected to come to, at the costs last quoted.
    pub cost: timada_core::Money,
    pub status: SupplierOrderStatus,
    /// What the supplier calls the order, once it has taken it.
    pub external_order_id: Option<String>,
}

fn purchase_projection<E: Executor>() -> Projection<E, SupplierOrderState> {
    Projection::new::<SupplierOrder>()
        .handler(on_purchase_drafted())
        .handler(on_purchase_placed())
        .handler(on_purchase_by_hand())
        .handler(on_purchase_refused())
        .handler(on_purchase_shipped())
        .handler(on_purchase_cancelled())
        .strict()
}

#[evento::handler]
async fn on_purchase_drafted(
    event: Event<SupplierOrderDrafted>,
    row: &mut SupplierOrderState,
) -> anyhow::Result<()> {
    row.id = event.aggregate_id.to_owned();
    row.order_id = event.data.order_id;
    row.supplier_id = event.data.supplier_id;
    row.lines = event.data.lines;
    row.ship_to = event.data.ship_to;
    row.cost = event.data.cost;
    row.status = SupplierOrderStatus::Drafted;
    Ok(())
}

#[evento::handler]
async fn on_purchase_placed(
    event: Event<SupplierOrderPlaced>,
    row: &mut SupplierOrderState,
) -> anyhow::Result<()> {
    row.status = SupplierOrderStatus::Placed;
    row.external_order_id = Some(event.data.external_order_id);
    Ok(())
}

#[evento::handler]
async fn on_purchase_by_hand(
    event: Event<SupplierOrderRecordedByHand>,
    row: &mut SupplierOrderState,
) -> anyhow::Result<()> {
    row.status = SupplierOrderStatus::Placed;
    row.external_order_id = Some(event.data.external_order_id);
    Ok(())
}

#[evento::handler]
async fn on_purchase_refused(
    _event: Event<SupplierOrderRefused>,
    row: &mut SupplierOrderState,
) -> anyhow::Result<()> {
    row.status = SupplierOrderStatus::Refused;
    Ok(())
}

#[evento::handler]
async fn on_purchase_shipped(
    _event: Event<SupplierOrderShipped>,
    row: &mut SupplierOrderState,
) -> anyhow::Result<()> {
    row.status = SupplierOrderStatus::Shipped;
    Ok(())
}

#[evento::handler]
async fn on_purchase_cancelled(
    _event: Event<SupplierOrderCancelled>,
    row: &mut SupplierOrderState,
) -> anyhow::Result<()> {
    row.status = SupplierOrderStatus::Cancelled;
    Ok(())
}
