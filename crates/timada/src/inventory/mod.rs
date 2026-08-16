//! Local stock for self-inventory products.
//!
//! A separate aggregate **co-keyed with the product id**: the first
//! `StockAdjusted` for a product starts an `Inventory` stream under the same
//! id, so stock churn never contends with the `Product` aggregate's version,
//! and read models can fold both streams per product id.

use anyhow::Result;
use evento::ProjectionAggregate;
use evento::metadata::Event;
use evento::projection::Projection;
use evento::sql::RwSqlite;

#[evento::aggregate]
pub enum Inventory {
    /// Stock was adjusted by `quantity_change` (restock, correction, damage…).
    /// `available` is the resulting quantity — derivable from the stream, but
    /// carried on the event so projections can set it absolutely and stay
    /// idempotent under replay.
    StockAdjusted {
        quantity_change: i64,
        available: i64,
        reason: String,
    },
}

/// Write-side state, replayed from the product's stock events.
#[evento::projection(bitcode::Encode, bitcode::Decode)]
#[derive(Debug)]
pub struct InventoryState {
    pub id: String,
    pub available: i64,
}

impl ProjectionAggregate for InventoryState {
    fn aggregate_id(&self) -> String {
        self.id.to_owned()
    }
}

#[evento::handler]
async fn on_stock_adjusted(event: Event<StockAdjusted>, state: &mut InventoryState) -> Result<()> {
    state.id = event.aggregate_id.clone();
    state.available = event.data.available;
    Ok(())
}

fn state_projection() -> Projection<RwSqlite, InventoryState> {
    Projection::new::<Inventory>()
        .handler(on_stock_adjusted())
        .strict()
}

/// Load the stock state for one product. `None` means no stock was ever
/// adjusted (treat as zero available).
pub async fn load(executor: &RwSqlite, product_id: &str) -> Result<Option<InventoryState>> {
    state_projection().load(product_id).execute(executor).await
}

/// Adjust local stock by `quantity_change` (positive or negative). Only
/// self-inventory products hold local stock; imported products' stock lives
/// at their provider. Available quantity can never go below zero.
///
/// Returns the resulting available quantity.
#[tracing::instrument(skip_all, fields(aggregate_id = %product_id, quantity_change))]
pub async fn adjust_stock(
    executor: &RwSqlite,
    product_id: &str,
    quantity_change: i64,
    reason: String,
) -> Result<i64> {
    let Some(product) = crate::product::load(executor, product_id).await? else {
        anyhow::bail!("product not found: {product_id}");
    };
    if product.provider_kind != crate::self_inventory::KIND {
        anyhow::bail!("stock for imported products is managed by their provider");
    }

    let state = load(executor, product_id).await?;
    let current = state.as_ref().map_or(0, |s| s.available);
    let Some(available) = current.checked_add(quantity_change) else {
        anyhow::bail!("stock adjustment overflows");
    };
    if available < 0 {
        anyhow::bail!("stock cannot go below zero (currently {current})");
    }

    let mut write = match state {
        Some(state) => state.write()?,
        // First stock event for this product: start the co-keyed stream.
        None => evento::append(product_id),
    };
    write
        .event(&StockAdjusted {
            quantity_change,
            available,
            reason,
        })
        .commit(executor)
        .await?;

    tracing::info!(available, "stock adjusted");
    Ok(available)
}
