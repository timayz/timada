//! Write side: the command gateway and the strict write models it loads.
//!
//! Every write model folds *all* events of its aggregate (`strict()`), so the
//! version it observes, and `write()` sends as `original_version`, never goes
//! stale. None of them is snapshotted: a product stream is a handful of events.

mod create;
mod edit;
mod lifecycle;
mod slug_index;

use std::fmt;
use std::ops::Deref;

pub use create::*;

use evento::{Executor, Projection, metadata::Event};

use crate::aggregator::{
    Archived, Claimed, Created, Described, Priced, Published, Released, Renamed, Unpublished,
};
use crate::error::CatalogError;
use crate::value_object::{format_money, product_slug_id};

/// The command gateway. Cheap to clone when `E` is.
pub struct Command<E: Executor>(pub E);

impl<E: Executor> Deref for Command<E> {
    type Target = E;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

// =============================================================================
// Product
// =============================================================================

/// Where a product stands: a draft is being prepared, a published one is on
/// sale, an archived one is gone for good.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ProductStatus {
    #[default]
    Draft,
    Published,
    Archived,
}

impl ProductStatus {
    /// The value stored in the read models.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::Published => "published",
            Self::Archived => "archived",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "draft" => Some(Self::Draft),
            "published" => Some(Self::Published),
            "archived" => Some(Self::Archived),
            _ => None,
        }
    }
}

impl fmt::Display for ProductStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A price: minor units of a currency.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Price {
    pub amount_cents: u64,
    pub currency: String,
}

impl fmt::Display for Price {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&format_money(self.amount_cents, &self.currency))
    }
}

#[evento::projection(id = id, name = "timada-catalog/ProductState")]
#[evento::snapshot(none)]
#[derive(Debug, PartialEq, Eq)]
pub struct ProductState {
    pub id: String,
    pub slug: String,
    pub name: String,
    pub description: String,
    pub price: Option<Price>,
    pub status: ProductStatus,
    pub created_by: String,
    /// Unix seconds of `Created`.
    pub created_at: u64,
    /// Unix seconds of the last event.
    pub updated_at: u64,
}

impl ProductState {
    pub fn is_published(&self) -> bool {
        self.status == ProductStatus::Published
    }

    pub fn is_archived(&self) -> bool {
        self.status == ProductStatus::Archived
    }
}

pub fn product_projection<E: Executor>() -> Projection<E, ProductState> {
    Projection::new::<crate::aggregator::Product>()
        .handler(on_created())
        .handler(on_renamed())
        .handler(on_described())
        .handler(on_priced())
        .handler(on_published())
        .handler(on_unpublished())
        .handler(on_archived())
        .strict()
}

#[evento::handler]
async fn on_created(event: Event<Created>, product: &mut ProductState) -> anyhow::Result<()> {
    product.id = event.aggregate_id.to_owned();
    product.slug = event.data.slug.to_owned();
    product.name = event.data.name.to_owned();
    product.created_by = event.data.created_by.to_owned();
    product.created_at = event.timestamp;
    product.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_renamed(event: Event<Renamed>, product: &mut ProductState) -> anyhow::Result<()> {
    product.name = event.data.name.to_owned();
    product.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_described(event: Event<Described>, product: &mut ProductState) -> anyhow::Result<()> {
    product.description = event.data.description.to_owned();
    product.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_priced(event: Event<Priced>, product: &mut ProductState) -> anyhow::Result<()> {
    product.price = Some(Price {
        amount_cents: event.data.amount_cents,
        currency: event.data.currency.to_owned(),
    });
    product.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_published(event: Event<Published>, product: &mut ProductState) -> anyhow::Result<()> {
    product.status = ProductStatus::Published;
    product.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_unpublished(
    event: Event<Unpublished>,
    product: &mut ProductState,
) -> anyhow::Result<()> {
    product.status = ProductStatus::Draft;
    product.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_archived(event: Event<Archived>, product: &mut ProductState) -> anyhow::Result<()> {
    product.status = ProductStatus::Archived;
    product.updated_at = event.timestamp;
    Ok(())
}

// =============================================================================
// ProductSlug (index)
// =============================================================================

#[evento::projection(id = id, name = "timada-catalog/ProductSlugState")]
#[evento::snapshot(none)]
#[derive(Debug, PartialEq, Eq)]
pub struct ProductSlugState {
    pub id: String,
    pub product_id: String,
    pub released: bool,
}

pub fn product_slug_projection<E: Executor>() -> Projection<E, ProductSlugState> {
    Projection::new::<crate::aggregator::ProductSlug>()
        .handler(on_slug_claimed())
        .handler(on_slug_released())
        .strict()
}

#[evento::handler]
async fn on_slug_claimed(
    event: Event<Claimed>,
    index: &mut ProductSlugState,
) -> anyhow::Result<()> {
    index.id = event.aggregate_id.to_owned();
    index.product_id = event.data.product_id.to_owned();
    index.released = false;
    Ok(())
}

#[evento::handler]
async fn on_slug_released(
    _event: Event<Released>,
    index: &mut ProductSlugState,
) -> anyhow::Result<()> {
    index.released = true;
    Ok(())
}

// =============================================================================
// Loads
// =============================================================================

impl<E: Executor> Command<E> {
    pub async fn load_product(&self, product_id: &str) -> anyhow::Result<Option<ProductState>> {
        product_projection().load(product_id).execute(&self.0).await
    }

    /// The product a (normalized) slug points at, if any.
    pub async fn load_product_by_slug(&self, slug: &str) -> anyhow::Result<Option<ProductState>> {
        let Some(index) = self.load_slug_index(slug).await? else {
            return Ok(None);
        };
        self.load_product(&index.product_id).await
    }

    /// The live index entry of a slug (released ones are `None`).
    pub async fn load_slug_index(&self, slug: &str) -> anyhow::Result<Option<ProductSlugState>> {
        Ok(self
            .load_slug_index_any(slug)
            .await?
            .filter(|index| !index.released))
    }

    /// The index entry including released ones (needed to re-claim).
    pub(crate) async fn load_slug_index_any(
        &self,
        slug: &str,
    ) -> anyhow::Result<Option<ProductSlugState>> {
        product_slug_projection()
            .load(product_slug_id(slug))
            .execute(&self.0)
            .await
    }

    /// The product, or `NotFound`; `Archived` once it is: every edit starts
    /// here.
    pub(crate) async fn require_editable(
        &self,
        product_id: &str,
    ) -> Result<ProductState, CatalogError> {
        let product = self
            .load_product(product_id)
            .await?
            .ok_or(CatalogError::NotFound)?;
        if product.is_archived() {
            return Err(CatalogError::Archived);
        }
        Ok(product)
    }
}
