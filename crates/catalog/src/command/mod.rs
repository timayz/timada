mod add_product_media;
mod archive_product;
mod categorise_product;
mod category;
mod create_product;
mod describe_product;
mod family;
mod label_product_energy;
mod specify_product;

use std::ops::Deref;

pub use category::{CreateCategory, MAX_CATEGORY_DEPTH, MAX_CATEGORY_FACETS};
pub use create_product::CreateProduct;
pub use describe_product::DescribeProduct;
pub use family::{CreateFamily, MAX_FAMILY_OPTIONS, MAX_OPTION_VALUES};

use evento::{Executor, Projection, metadata::Event};

use crate::{
    aggregator::{
        Category, CategoryArchived, CategoryCreated, CategoryDescribed, CategoryFacetsDefined,
        CategoryMoved, CategoryPositioned, CategoryRenamed, FamilyCreated, FamilyDissolved,
        FamilyOptionsDefined, FamilyRenamed, FamilyVariantPlaced, FamilyVariantRemoved, Product,
        ProductArchived, ProductCategorised, ProductCreated, ProductDescribed,
        ProductEnergyLabelled, ProductFamily, ProductJoinedFamily, ProductLeftFamily,
        ProductMediaAdded, ProductSpecified,
    },
    error::CatalogError,
    value_object::{FamilyOption, OptionValue, SpecKey},
};

/// Deterministic product id: one product per SKU.
pub fn product_id(sku: &str) -> String {
    timada_core::id::derived(&[sku], "product")
}

/// Deterministic category id: one category per slug.
pub fn category_id(slug: &str) -> String {
    timada_core::id::derived(&[slug], "category")
}

/// Deterministic family id: one product family per slug.
pub fn family_id(slug: &str) -> String {
    timada_core::id::derived(&[slug], "product-family")
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
    pub category_id: Option<String>,
    /// The family the product is a variant of, if any.
    pub family_id: Option<String>,
}

// Strict + explicit skips: a non-strict projection only *reads* the events it
// handles, so the version it observes (and `write()` relies on) would go stale.
fn create_projection<E: Executor>() -> Projection<E, ProductState> {
    Projection::new::<Product>()
        .handler(on_product_created())
        .handler(on_product_archived())
        .handler(on_product_categorised())
        .handler(on_product_joined_family())
        .handler(on_product_left_family())
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

#[evento::handler]
async fn on_product_categorised(
    event: Event<ProductCategorised>,
    row: &mut ProductState,
) -> anyhow::Result<()> {
    row.category_id = Some(event.data.category_id);
    Ok(())
}

#[evento::handler]
async fn on_product_joined_family(
    event: Event<ProductJoinedFamily>,
    row: &mut ProductState,
) -> anyhow::Result<()> {
    row.family_id = Some(event.data.family_id);
    Ok(())
}

#[evento::handler]
async fn on_product_left_family(
    _event: Event<ProductLeftFamily>,
    row: &mut ProductState,
) -> anyhow::Result<()> {
    row.family_id = None;
    Ok(())
}

/// Write-side state of a category.
#[evento::projection(id = id)]
#[evento::snapshot(none)]
pub struct CategoryState {
    pub id: String,
    pub slug: String,
    pub name: String,
    pub description: String,
    pub parent_id: Option<String>,
    pub position: u32,
    pub archived: bool,
    /// The specs shoppers filter the category by; empty: its parent's.
    pub facets: Vec<SpecKey>,
}

// Every event is folded, so the version `write()` relies on is exact.
fn create_category_projection<E: Executor>() -> Projection<E, CategoryState> {
    Projection::new::<Category>()
        .handler(on_category_created())
        .handler(on_category_renamed())
        .handler(on_category_described())
        .handler(on_category_moved())
        .handler(on_category_positioned())
        .handler(on_category_archived())
        .handler(on_category_facets_defined())
        .strict()
}

impl<E: Executor> Command<'_, E> {
    pub async fn load_category(
        &self,
        id: impl Into<String>,
    ) -> anyhow::Result<Option<CategoryState>> {
        create_category_projection().load(id).execute(self.0).await
    }

    /// Loads a category that must exist and not be archived.
    async fn load_open_category(
        &self,
        id: impl Into<String>,
    ) -> Result<CategoryState, CatalogError> {
        let Some(category) = self.load_category(id).await? else {
            return Err(CatalogError::CategoryNotFound);
        };
        if category.archived {
            return Err(CatalogError::CategoryArchived);
        }
        Ok(category)
    }
}

#[evento::handler]
async fn on_category_created(
    event: Event<CategoryCreated>,
    row: &mut CategoryState,
) -> anyhow::Result<()> {
    row.id = event.aggregate_id.to_owned();
    row.slug = event.data.slug;
    row.name = event.data.name;
    row.parent_id = event.data.parent_id;
    Ok(())
}

#[evento::handler]
async fn on_category_renamed(
    event: Event<CategoryRenamed>,
    row: &mut CategoryState,
) -> anyhow::Result<()> {
    row.name = event.data.name;
    Ok(())
}

#[evento::handler]
async fn on_category_described(
    event: Event<CategoryDescribed>,
    row: &mut CategoryState,
) -> anyhow::Result<()> {
    row.description = event.data.description;
    Ok(())
}

#[evento::handler]
async fn on_category_moved(
    event: Event<CategoryMoved>,
    row: &mut CategoryState,
) -> anyhow::Result<()> {
    row.parent_id = event.data.parent_id;
    Ok(())
}

#[evento::handler]
async fn on_category_positioned(
    event: Event<CategoryPositioned>,
    row: &mut CategoryState,
) -> anyhow::Result<()> {
    row.position = event.data.position;
    Ok(())
}

#[evento::handler]
async fn on_category_archived(
    _event: Event<CategoryArchived>,
    row: &mut CategoryState,
) -> anyhow::Result<()> {
    row.archived = true;
    Ok(())
}

#[evento::handler]
async fn on_category_facets_defined(
    event: Event<CategoryFacetsDefined>,
    row: &mut CategoryState,
) -> anyhow::Result<()> {
    row.facets = event.data.facets;
    Ok(())
}

/// A product's place in its family: one value per option.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FamilyVariant {
    pub product_id: String,
    pub values: Vec<OptionValue>,
}

impl FamilyVariant {
    /// Where the variant stands on `option`, if it was said.
    pub fn value_of(&self, option: &str) -> Option<&str> {
        self.values
            .iter()
            .find(|placed| placed.option == option)
            .map(|placed| placed.value.as_str())
    }
}

/// A product family as its events leave it: the options, and the variants in
/// the order they joined. Read straight from the store — a product page never
/// waits on a read model to offer the other versions.
#[evento::projection(id = id)]
#[evento::snapshot(none)]
pub struct FamilyState {
    pub id: String,
    pub slug: String,
    pub name: String,
    pub options: Vec<FamilyOption>,
    pub variants: Vec<FamilyVariant>,
    pub dissolved: bool,
}

impl FamilyState {
    pub fn variant(&self, product_id: &str) -> Option<&FamilyVariant> {
        self.variants
            .iter()
            .find(|variant| variant.product_id == product_id)
    }

    /// Whether the variant has a value, still offered, for every option — an
    /// option added after it joined leaves it to complete.
    pub fn is_complete(&self, variant: &FamilyVariant) -> bool {
        self.options.iter().all(|option| {
            variant
                .value_of(&option.name)
                .is_some_and(|value| option.values.iter().any(|offered| offered == value))
        })
    }
}

// Every event is folded, so the version `write()` relies on is exact.
fn create_family_projection<E: Executor>() -> Projection<E, FamilyState> {
    Projection::new::<ProductFamily>()
        .handler(on_family_created())
        .handler(on_family_renamed())
        .handler(on_family_options_defined())
        .handler(on_family_variant_placed())
        .handler(on_family_variant_removed())
        .handler(on_family_dissolved())
        .strict()
}

impl<E: Executor> Command<'_, E> {
    pub async fn load_family(&self, id: impl Into<String>) -> anyhow::Result<Option<FamilyState>> {
        create_family_projection().load(id).execute(self.0).await
    }

    /// Loads a family that must exist and not be dissolved.
    async fn load_open_family(&self, id: impl Into<String>) -> Result<FamilyState, CatalogError> {
        let Some(family) = self.load_family(id).await? else {
            return Err(CatalogError::FamilyNotFound);
        };
        if family.dissolved {
            return Err(CatalogError::FamilyDissolved);
        }
        Ok(family)
    }
}

#[evento::handler]
async fn on_family_created(
    event: Event<FamilyCreated>,
    row: &mut FamilyState,
) -> anyhow::Result<()> {
    row.id = event.aggregate_id.to_owned();
    row.slug = event.data.slug;
    row.name = event.data.name;
    Ok(())
}

#[evento::handler]
async fn on_family_renamed(
    event: Event<FamilyRenamed>,
    row: &mut FamilyState,
) -> anyhow::Result<()> {
    row.name = event.data.name;
    Ok(())
}

#[evento::handler]
async fn on_family_options_defined(
    event: Event<FamilyOptionsDefined>,
    row: &mut FamilyState,
) -> anyhow::Result<()> {
    row.options = event.data.options;
    Ok(())
}

#[evento::handler]
async fn on_family_variant_placed(
    event: Event<FamilyVariantPlaced>,
    row: &mut FamilyState,
) -> anyhow::Result<()> {
    let FamilyVariantPlaced { product_id, values } = event.data;
    match row
        .variants
        .iter_mut()
        .find(|variant| variant.product_id == product_id)
    {
        Some(variant) => variant.values = values,
        None => row.variants.push(FamilyVariant { product_id, values }),
    }
    Ok(())
}

#[evento::handler]
async fn on_family_variant_removed(
    event: Event<FamilyVariantRemoved>,
    row: &mut FamilyState,
) -> anyhow::Result<()> {
    row.variants
        .retain(|variant| variant.product_id != event.data.product_id);
    Ok(())
}

#[evento::handler]
async fn on_family_dissolved(
    _event: Event<FamilyDissolved>,
    row: &mut FamilyState,
) -> anyhow::Result<()> {
    row.dissolved = true;
    Ok(())
}
