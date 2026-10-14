//! Write side: the command gateway and the strict write models it loads.
//!
//! Every write model folds *all* events of its aggregate (`strict()`), so the
//! version it observes, and `write()` sends as `original_version`, never goes
//! stale. None of them is snapshotted: a product stream is a handful of events.

mod attribute;
mod categorize;
mod category;
mod category_slug_index;
mod create;
mod edit;
mod image;
mod lifecycle;
mod option;
mod sku_index;
mod slug_index;
mod variant;

use std::collections::BTreeSet;
use std::fmt;
use std::ops::Deref;

pub use category::*;
pub use create::*;
pub use image::*;
pub use variant::*;

use evento::{Executor, Projection, metadata::Event};

use crate::aggregator::category::{
    Archived as CategoryArchived, Claimed as CategorySlugClaimed, Created as CategoryCreated,
    Described as CategoryDescribed, Moved as CategoryMoved, Positioned as CategoryPositioned,
    Released as CategorySlugReleased, Renamed as CategoryRenamed,
};
use crate::aggregator::sku::{Claimed as SkuClaimed, Released as SkuReleased};
use crate::aggregator::{
    Archived, AttributeRemoved, AttributeSet, Categorized, Claimed, Created, Described, ImageAdded,
    ImageAltChanged, ImageRemoved, ImagesReordered, OptionDefined, OptionRemoved, OptionRenamed,
    OptionValuesChanged, Priced, Published, Released, Renamed, TaxCategorySet, Uncategorized,
    Unpublished, VariantDefined, VariantImaged, VariantPriced, VariantRemoved, VariantSkuChanged,
};
use crate::error::CatalogError;
use crate::value_object::{
    MAX_CATEGORY_DEPTH, OptionSelection, category_slug_id, format_money, product_sku_id,
    product_slug_id,
};

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

/// A way a product varies, and the values it takes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProductOption {
    pub id: String,
    pub name: String,
    pub values: Vec<String>,
}

/// One combination of the options, with its own price: what a shopper buys.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProductVariant {
    pub id: String,
    /// A value for every option of the product, in the options' order.
    pub selections: Vec<OptionSelection>,
    pub sku: Option<String>,
    pub price: Price,
    /// One of the product's pictures, shown when this variant is picked.
    pub image_id: Option<String>,
}

impl ProductVariant {
    /// Whether this variant is the one those selections name, whatever
    /// order they come in.
    pub fn matches(&self, selections: &[OptionSelection]) -> bool {
        self.selections.len() == selections.len()
            && self.selections.iter().all(|mine| selections.contains(mine))
    }

    /// The value it takes for an option, if it names one.
    pub fn value_of(&self, option_id: &str) -> Option<&str> {
        self.selections
            .iter()
            .find(|s| s.option_id == option_id)
            .map(|s| s.value.as_str())
    }
}

/// A fact about a product, for the table beside its description.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProductAttribute {
    pub name: String,
    pub value: String,
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
    /// The ways it varies, in the order a page shows the menus.
    pub options: Vec<ProductOption>,
    /// The combinations it sells, each with its own price.
    pub variants: Vec<ProductVariant>,
    /// The facts in its table, in the order they were set.
    pub attributes: Vec<ProductAttribute>,
    /// What kind of goods it is for VAT; `None` is `standard`.
    pub tax_category: Option<String>,
    pub created_by: String,
    /// Unix seconds of `Created`.
    pub created_at: u64,
    /// Unix seconds of the last event.
    pub updated_at: u64,
}

impl ProductState {
    /// What kind of goods it is for VAT, `standard` unless told otherwise.
    pub fn tax_category(&self) -> &str {
        self.tax_category
            .as_deref()
            .unwrap_or(crate::TAX_CATEGORY_STANDARD)
    }

    pub fn is_published(&self) -> bool {
        self.status == ProductStatus::Published
    }

    pub fn is_archived(&self) -> bool {
        self.status == ProductStatus::Archived
    }

    /// The currency everything about this product is priced in, once
    /// anything is: its own price, else its first variant's.
    pub fn currency(&self) -> Option<&str> {
        self.price
            .as_ref()
            .map(|price| price.currency.as_str())
            .or_else(|| self.variants.first().map(|v| v.price.currency.as_str()))
    }

    /// Whether a shopper could be charged for it: a plain price, or at least
    /// one variant to pick.
    pub fn is_priced(&self) -> bool {
        self.price.is_some() || !self.variants.is_empty()
    }

    /// What a listing shows: the cheapest and dearest a shopper may pay. The
    /// two are equal when nothing varies.
    pub fn price_range(&self) -> Option<(u64, u64, &str)> {
        if self.variants.is_empty() {
            let price = self.price.as_ref()?;
            return Some((price.amount_cents, price.amount_cents, &price.currency));
        }
        let currency = self.currency()?;
        let amounts = self.variants.iter().map(|v| v.price.amount_cents);
        Some((amounts.clone().min()?, amounts.max()?, currency))
    }

    /// The variant those selections name, if it is one the product sells.
    pub fn variant_for(&self, selections: &[OptionSelection]) -> Option<&ProductVariant> {
        self.variants.iter().find(|v| v.matches(selections))
    }

    /// One of the ways it varies, by id.
    pub fn option(&self, option_id: &str) -> Option<&ProductOption> {
        self.options.iter().find(|o| o.id == option_id)
    }

    /// One of the combinations it sells, by id.
    pub fn variant(&self, variant_id: &str) -> Option<&ProductVariant> {
        self.variants.iter().find(|v| v.id == variant_id)
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
        .handler(on_option_defined())
        .handler(on_option_renamed())
        .handler(on_option_values_changed())
        .handler(on_option_removed())
        .handler(on_variant_defined())
        .handler(on_variant_priced())
        .handler(on_variant_sku_changed())
        .handler(on_variant_imaged())
        .handler(on_variant_removed())
        .handler(on_attribute_set())
        .handler(on_attribute_removed())
        .handler(on_tax_category_set())
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
    // A picture that goes takes its place on any variant that showed it.
    for variant in &mut product.variants {
        if variant.image_id.as_deref() == Some(event.data.image_id.as_str()) {
            variant.image_id = None;
        }
    }
    product.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_option_defined(
    event: Event<OptionDefined>,
    product: &mut ProductState,
) -> anyhow::Result<()> {
    product.options.push(ProductOption {
        id: event.data.option_id.to_owned(),
        name: event.data.name.to_owned(),
        values: event.data.values.to_owned(),
    });
    product.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_option_renamed(
    event: Event<OptionRenamed>,
    product: &mut ProductState,
) -> anyhow::Result<()> {
    if let Some(option) = product
        .options
        .iter_mut()
        .find(|option| option.id == event.data.option_id)
    {
        option.name = event.data.name.to_owned();
    }
    product.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_option_values_changed(
    event: Event<OptionValuesChanged>,
    product: &mut ProductState,
) -> anyhow::Result<()> {
    if let Some(option) = product
        .options
        .iter_mut()
        .find(|option| option.id == event.data.option_id)
    {
        option.values = event.data.values.to_owned();
    }
    product.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_option_removed(
    event: Event<OptionRemoved>,
    product: &mut ProductState,
) -> anyhow::Result<()> {
    product
        .options
        .retain(|option| option.id != event.data.option_id);
    product.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_variant_defined(
    event: Event<VariantDefined>,
    product: &mut ProductState,
) -> anyhow::Result<()> {
    product.variants.push(ProductVariant {
        id: event.data.variant_id.to_owned(),
        selections: event.data.selections.to_owned(),
        sku: event.data.sku.to_owned(),
        price: Price {
            amount_cents: event.data.amount_cents,
            currency: event.data.currency.to_owned(),
        },
        image_id: event.data.image_id.to_owned(),
    });
    product.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_variant_priced(
    event: Event<VariantPriced>,
    product: &mut ProductState,
) -> anyhow::Result<()> {
    if let Some(variant) = product
        .variants
        .iter_mut()
        .find(|variant| variant.id == event.data.variant_id)
    {
        variant.price = Price {
            amount_cents: event.data.amount_cents,
            currency: event.data.currency.to_owned(),
        };
    }
    product.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_variant_sku_changed(
    event: Event<VariantSkuChanged>,
    product: &mut ProductState,
) -> anyhow::Result<()> {
    if let Some(variant) = product
        .variants
        .iter_mut()
        .find(|variant| variant.id == event.data.variant_id)
    {
        variant.sku = event.data.sku.to_owned();
    }
    product.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_variant_imaged(
    event: Event<VariantImaged>,
    product: &mut ProductState,
) -> anyhow::Result<()> {
    if let Some(variant) = product
        .variants
        .iter_mut()
        .find(|variant| variant.id == event.data.variant_id)
    {
        variant.image_id = event.data.image_id.to_owned();
    }
    product.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_variant_removed(
    event: Event<VariantRemoved>,
    product: &mut ProductState,
) -> anyhow::Result<()> {
    product
        .variants
        .retain(|variant| variant.id != event.data.variant_id);
    product.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_attribute_set(
    event: Event<AttributeSet>,
    product: &mut ProductState,
) -> anyhow::Result<()> {
    match product
        .attributes
        .iter_mut()
        .find(|attribute| attribute.name == event.data.name)
    {
        Some(attribute) => attribute.value = event.data.value.to_owned(),
        None => product.attributes.push(ProductAttribute {
            name: event.data.name.to_owned(),
            value: event.data.value.to_owned(),
        }),
    }
    product.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_attribute_removed(
    event: Event<AttributeRemoved>,
    product: &mut ProductState,
) -> anyhow::Result<()> {
    product
        .attributes
        .retain(|attribute| attribute.name != event.data.name);
    product.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_tax_category_set(
    event: Event<TaxCategorySet>,
    product: &mut ProductState,
) -> anyhow::Result<()> {
    product.tax_category = event.data.category.to_owned();
    product.updated_at = event.timestamp;
    Ok(())
}

// =============================================================================
// ProductSku (index)
// =============================================================================

#[evento::projection(id = id, name = "timada-catalog/ProductSkuState")]
#[evento::snapshot(none)]
#[derive(Debug, PartialEq, Eq)]
pub struct ProductSkuState {
    pub id: String,
    pub product_id: String,
    pub variant_id: String,
    pub released: bool,
}

pub fn product_sku_projection<E: Executor>() -> Projection<E, ProductSkuState> {
    Projection::new::<crate::aggregator::sku::ProductSku>()
        .handler(on_sku_claimed())
        .handler(on_sku_released())
        .strict()
}

#[evento::handler]
async fn on_sku_claimed(
    event: Event<SkuClaimed>,
    index: &mut ProductSkuState,
) -> anyhow::Result<()> {
    index.id = event.aggregate_id.to_owned();
    index.product_id = event.data.product_id.to_owned();
    index.variant_id = event.data.variant_id.to_owned();
    index.released = false;
    Ok(())
}

#[evento::handler]
async fn on_sku_released(
    _event: Event<SkuReleased>,
    index: &mut ProductSkuState,
) -> anyhow::Result<()> {
    index.released = true;
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

    /// The live index entry of a stock code (released ones are `None`).
    pub async fn load_sku_index(&self, sku: &str) -> anyhow::Result<Option<ProductSkuState>> {
        Ok(self
            .load_sku_index_any(sku)
            .await?
            .filter(|index| !index.released))
    }

    pub(crate) async fn load_sku_index_any(
        &self,
        sku: &str,
    ) -> anyhow::Result<Option<ProductSkuState>> {
        product_sku_projection()
            .load(product_sku_id(sku))
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
