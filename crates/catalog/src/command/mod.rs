mod add_product_media;
mod archive_product;
mod create_product;
mod describe_product;
mod label_product_energy;
mod specify_product;

use std::ops::Deref;

pub use create_product::CreateProduct;
pub use describe_product::DescribeProduct;

use evento::{Executor, Projection, metadata::Event};

use crate::{
    aggregator::{
        Product, ProductArchived, ProductCreated, ProductDescribed, ProductEnergyLabelled,
        ProductMediaAdded, ProductSpecified,
    },
    error::CatalogError,
};

/// Deterministic product id: one product per SKU.
pub fn product_id(sku: &str) -> String {
    timada_core::id::derived(&[sku], "product")
}

pub struct Command<'a, E: Executor>(pub &'a E);

impl<E: Executor> Deref for Command<'_, E> {
    type Target = E;

    fn deref(&self) -> &Self::Target {
        self.0
    }
}

impl<E: Executor> Command<'_, E> {
    pub async fn load(&self, id: impl Into<String>) -> anyhow::Result<Option<ProductState>> {
        create_projection().load(id).execute(self.0).await
    }

    /// Loads a product that must exist and not be archived.
    async fn load_active(&self, id: impl Into<String>) -> Result<ProductState, CatalogError> {
        let Some(product) = self.load(id).await? else {
            return Err(CatalogError::ProductNotFound);
        };
        if product.archived {
            return Err(CatalogError::ProductArchived);
        }
        Ok(product)
    }
}

/// Write-side state: just enough to guard the commands.
#[evento::projection(id = id)]
#[evento::snapshot(none)]
pub struct ProductState {
    pub id: String,
    pub sku: String,
    pub archived: bool,
}

// Strict + explicit skips: a non-strict projection only *reads* the events it
// handles, so the version it observes (and `write()` relies on) would go stale.
fn create_projection<E: Executor>() -> Projection<E, ProductState> {
    Projection::new::<Product>()
        .handler(on_product_created())
        .handler(on_product_archived())
        .skip::<ProductDescribed>()
        .skip::<ProductSpecified>()
        .skip::<ProductMediaAdded>()
        .skip::<ProductEnergyLabelled>()
        .strict()
}

#[evento::handler]
async fn on_product_created(
    event: Event<ProductCreated>,
    row: &mut ProductState,
) -> anyhow::Result<()> {
    row.id = event.aggregate_id.to_owned();
    row.sku = event.data.sku;
    Ok(())
}

#[evento::handler]
async fn on_product_archived(
    _event: Event<ProductArchived>,
    row: &mut ProductState,
) -> anyhow::Result<()> {
    row.archived = true;
    Ok(())
}
