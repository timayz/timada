//! Write side: the command gateway and the strict write models it loads.
//!
//! Every write model folds *all* events of its aggregate (`strict()`), so the
//! version it observes, and `write()` sends as `original_version`, never goes
//! stale. None of them is snapshotted: a product stream is a handful of events.

mod categorize;
mod category;
mod category_slug_index;
mod create;
mod edit;
mod image;
mod lifecycle;
mod slug_index;

use std::collections::BTreeSet;
use std::fmt;
use std::ops::Deref;

pub use category::*;
pub use create::*;
pub use image::*;

use evento::{Executor, Projection, metadata::Event};

use crate::aggregator::category::{
    Archived as CategoryArchived, Claimed as CategorySlugClaimed, Created as CategoryCreated,
    Described as CategoryDescribed, Moved as CategoryMoved, Positioned as CategoryPositioned,
    Released as CategorySlugReleased, Renamed as CategoryRenamed,
};
use crate::aggregator::{
    Archived, Categorized, Claimed, Created, Described, ImageAdded, ImageAltChanged, ImageRemoved,
    ImagesReordered, Priced, Published, Released, Renamed, Uncategorized, Unpublished,
};
use crate::error::CatalogError;
use crate::value_object::{MAX_CATEGORY_DEPTH, category_slug_id, format_money, product_slug_id};

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

/// A picture of a product. The bytes live outside the store; `file` is what
/// the host serves them under.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProductImage {
    pub id: String,
    /// Lowercase, no dot.
    pub ext: String,
    pub mime: String,
    /// In bytes.
    pub size: u64,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub alt: String,
}

impl ProductImage {
    /// The file name the bytes are stored under: `<id>.<ext>`.
    pub fn file(&self) -> String {
        format!("{}.{}", self.id, self.ext)
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
    /// The categories it belongs to, by id.
    pub categories: BTreeSet<String>,
    /// Its pictures, in the order they are shown; the first is the one a
    /// listing uses.
    pub images: Vec<ProductImage>,
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
        .handler(on_categorized())
        .handler(on_uncategorized())
        .handler(on_image_added())
        .handler(on_image_alt_changed())
        .handler(on_images_reordered())
        .handler(on_image_removed())
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

#[evento::handler]
async fn on_categorized(
    event: Event<Categorized>,
    product: &mut ProductState,
) -> anyhow::Result<()> {
    product.categories.insert(event.data.category_id.to_owned());
    product.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_uncategorized(
    event: Event<Uncategorized>,
    product: &mut ProductState,
) -> anyhow::Result<()> {
    product.categories.remove(&event.data.category_id);
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

#[evento::handler]
async fn on_image_added(
    event: Event<ImageAdded>,
    product: &mut ProductState,
) -> anyhow::Result<()> {
    product.images.push(ProductImage {
        id: event.data.image_id.to_owned(),
        ext: event.data.ext.to_owned(),
        mime: event.data.mime.to_owned(),
        size: event.data.size,
        width: event.data.width,
        height: event.data.height,
        alt: event.data.alt.to_owned(),
    });
    product.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_image_alt_changed(
    event: Event<ImageAltChanged>,
    product: &mut ProductState,
) -> anyhow::Result<()> {
    if let Some(image) = product
        .images
        .iter_mut()
        .find(|image| image.id == event.data.image_id)
    {
        image.alt = event.data.alt.to_owned();
    }
    product.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_images_reordered(
    event: Event<ImagesReordered>,
    product: &mut ProductState,
) -> anyhow::Result<()> {
    // Anything the order leaves out keeps its place at the end, so a reorder
    // written against an older gallery cannot lose a picture.
    product.images.sort_by_key(|image| {
        event
            .data
            .image_ids
            .iter()
            .position(|id| *id == image.id)
            .unwrap_or(usize::MAX)
    });
    product.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_image_removed(
    event: Event<ImageRemoved>,
    product: &mut ProductState,
) -> anyhow::Result<()> {
    product
        .images
        .retain(|image| image.id != event.data.image_id);
    product.updated_at = event.timestamp;
    Ok(())
}

// =============================================================================
// Category
// =============================================================================

#[evento::projection(id = id, name = "timada-catalog/CategoryState")]
#[evento::snapshot(none)]
#[derive(Debug, PartialEq, Eq)]
pub struct CategoryState {
    pub id: String,
    pub slug: String,
    pub name: String,
    pub description: String,
    /// `None` for a top-level category.
    pub parent_id: Option<String>,
    /// Where it sorts among its siblings, lowest first.
    pub position: u32,
    pub archived: bool,
    pub created_by: String,
    /// Unix seconds of `Created`.
    pub created_at: u64,
    /// Unix seconds of the last event.
    pub updated_at: u64,
}

pub fn category_projection<E: Executor>() -> Projection<E, CategoryState> {
    Projection::new::<crate::aggregator::category::Category>()
        .handler(on_category_created())
        .handler(on_category_renamed())
        .handler(on_category_described())
        .handler(on_category_moved())
        .handler(on_category_positioned())
        .handler(on_category_archived())
        .strict()
}

#[evento::handler]
async fn on_category_created(
    event: Event<CategoryCreated>,
    category: &mut CategoryState,
) -> anyhow::Result<()> {
    category.id = event.aggregate_id.to_owned();
    category.slug = event.data.slug.to_owned();
    category.name = event.data.name.to_owned();
    category.parent_id = event.data.parent_id.to_owned();
    category.created_by = event.data.created_by.to_owned();
    category.created_at = event.timestamp;
    category.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_category_renamed(
    event: Event<CategoryRenamed>,
    category: &mut CategoryState,
) -> anyhow::Result<()> {
    category.name = event.data.name.to_owned();
    category.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_category_described(
    event: Event<CategoryDescribed>,
    category: &mut CategoryState,
) -> anyhow::Result<()> {
    category.description = event.data.description.to_owned();
    category.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_category_moved(
    event: Event<CategoryMoved>,
    category: &mut CategoryState,
) -> anyhow::Result<()> {
    category.parent_id = event.data.parent_id.to_owned();
    category.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_category_positioned(
    event: Event<CategoryPositioned>,
    category: &mut CategoryState,
) -> anyhow::Result<()> {
    category.position = event.data.position;
    category.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_category_archived(
    event: Event<CategoryArchived>,
    category: &mut CategoryState,
) -> anyhow::Result<()> {
    category.archived = true;
    category.updated_at = event.timestamp;
    Ok(())
}

// =============================================================================
// CategorySlug (index)
// =============================================================================

#[evento::projection(id = id, name = "timada-catalog/CategorySlugState")]
#[evento::snapshot(none)]
#[derive(Debug, PartialEq, Eq)]
pub struct CategorySlugState {
    pub id: String,
    pub category_id: String,
    pub released: bool,
}

pub fn category_slug_projection<E: Executor>() -> Projection<E, CategorySlugState> {
    Projection::new::<crate::aggregator::category::CategorySlug>()
        .handler(on_category_slug_claimed())
        .handler(on_category_slug_released())
        .strict()
}

#[evento::handler]
async fn on_category_slug_claimed(
    event: Event<CategorySlugClaimed>,
    index: &mut CategorySlugState,
) -> anyhow::Result<()> {
    index.id = event.aggregate_id.to_owned();
    index.category_id = event.data.category_id.to_owned();
    index.released = false;
    Ok(())
}

#[evento::handler]
async fn on_category_slug_released(
    _event: Event<CategorySlugReleased>,
    index: &mut CategorySlugState,
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

impl<E: Executor> Command<E> {
    pub async fn load_category(&self, category_id: &str) -> anyhow::Result<Option<CategoryState>> {
        category_projection()
            .load(category_id)
            .execute(&self.0)
            .await
    }

    /// The category a (normalized) slug points at, if any.
    pub async fn load_category_by_slug(&self, slug: &str) -> anyhow::Result<Option<CategoryState>> {
        let Some(index) = self.load_category_slug_index(slug).await? else {
            return Ok(None);
        };
        self.load_category(&index.category_id).await
    }

    /// The live index entry of a category slug (released ones are `None`).
    pub async fn load_category_slug_index(
        &self,
        slug: &str,
    ) -> anyhow::Result<Option<CategorySlugState>> {
        Ok(self
            .load_category_slug_index_any(slug)
            .await?
            .filter(|index| !index.released))
    }

    pub(crate) async fn load_category_slug_index_any(
        &self,
        slug: &str,
    ) -> anyhow::Result<Option<CategorySlugState>> {
        category_slug_projection()
            .load(category_slug_id(slug))
            .execute(&self.0)
            .await
    }

    /// The category, as long as it still takes edits.
    pub(crate) async fn require_live_category(
        &self,
        category_id: &str,
    ) -> Result<CategoryState, CatalogError> {
        let category = self
            .load_category(category_id)
            .await?
            .ok_or(CatalogError::CategoryNotFound)?;
        if category.archived {
            return Err(CatalogError::CategoryArchived);
        }
        Ok(category)
    }

    /// The chain above a category, top-level first: empty for a top-level
    /// one. Stops at a missing parent, and at a loop after a few more steps
    /// than the tree allows (the commands refuse loops; this only guards the
    /// walk).
    pub async fn ancestors(&self, category_id: &str) -> anyhow::Result<Vec<CategoryState>> {
        let mut chain = Vec::new();
        let mut next = self
            .load_category(category_id)
            .await?
            .and_then(|category| category.parent_id);
        while let Some(parent_id) = next {
            if chain.len() > MAX_CATEGORY_DEPTH * 2
                || chain
                    .iter()
                    .any(|seen: &CategoryState| seen.id == parent_id)
                || parent_id == category_id
            {
                break;
            }
            let Some(parent) = self.load_category(&parent_id).await? else {
                break;
            };
            next = parent.parent_id.clone();
            chain.push(parent);
        }
        chain.reverse();
        Ok(chain)
    }
}
