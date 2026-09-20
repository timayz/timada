//! Everything the product page needs from the catalog, folded from one
//! `Product` stream. Executor-backed snapshots via the bitcode derives.

use evento::{Executor, metadata::Event, projection::Projection};

use crate::{
    aggregator::{
        Product, ProductArchived, ProductCategorised, ProductCreated, ProductDescribed,
        ProductEnergyLabelled, ProductMediaAdded, ProductSpecified,
    },
    value_object::{Brand, EnergyClass, Media, Spec},
};

#[evento::projection(bitcode::Encode, bitcode::Decode)]
#[derive(Debug, PartialEq)]
pub struct ProductPageView {
    pub id: String,
    pub sku: String,
    pub name: String,
    pub brand: Brand,
    /// The label the product was created with; see `category_id`.
    pub category_path: Vec<String>,
    /// The category the product is filed under, once it has been.
    pub category_id: Option<String>,
    pub short_description: String,
    pub long_description: String,
    pub key_features: Vec<String>,
    pub specs: Vec<Spec>,
    pub media: Vec<Media>,
    pub energy_class: Option<EnergyClass>,
    pub energy_info_sheet_url: Option<String>,
    pub warranty_months: u16,
    pub archived: bool,
}

pub fn create_projection<E: Executor>() -> Projection<E, ProductPageView> {
    Projection::new::<Product>()
        .handler(on_product_created())
        .handler(on_product_described())
        .handler(on_product_specified())
        .handler(on_product_media_added())
        .handler(on_product_energy_labelled())
        .handler(on_product_archived())
        .handler(on_product_categorised())
        .strict()
        // `category_id` joined the snapshot.
        .revision(1)
}

pub async fn load<E: Executor>(
    executor: &E,
    id: impl Into<String>,
) -> anyhow::Result<Option<ProductPageView>> {
    create_projection().load(id).execute(executor).await
}

#[evento::handler]
async fn on_product_created(
    event: Event<ProductCreated>,
    row: &mut ProductPageView,
) -> anyhow::Result<()> {
    row.id = event.aggregate_id.to_owned();
    row.sku = event.data.sku;
    row.name = event.data.name;
    row.brand = event.data.brand;
    row.category_path = event.data.category_path;
    row.short_description = event.data.short_description;
    row.warranty_months = event.data.warranty_months;
    Ok(())
}

#[evento::handler]
async fn on_product_described(
    event: Event<ProductDescribed>,
    row: &mut ProductPageView,
) -> anyhow::Result<()> {
    row.long_description = event.data.long_description;
    row.key_features = event.data.key_features;
    Ok(())
}

#[evento::handler]
async fn on_product_specified(
    event: Event<ProductSpecified>,
    row: &mut ProductPageView,
) -> anyhow::Result<()> {
    row.specs = event.data.specs;
    Ok(())
}

#[evento::handler]
async fn on_product_media_added(
    event: Event<ProductMediaAdded>,
    row: &mut ProductPageView,
) -> anyhow::Result<()> {
    row.media.push(event.data.media);
    Ok(())
}

#[evento::handler]
async fn on_product_energy_labelled(
    event: Event<ProductEnergyLabelled>,
    row: &mut ProductPageView,
) -> anyhow::Result<()> {
    row.energy_class = Some(event.data.class);
    row.energy_info_sheet_url = Some(event.data.info_sheet_url);
    Ok(())
}

#[evento::handler]
async fn on_product_archived(
    _event: Event<ProductArchived>,
    row: &mut ProductPageView,
) -> anyhow::Result<()> {
    row.archived = true;
    Ok(())
}

#[evento::handler]
async fn on_product_categorised(
    event: Event<ProductCategorised>,
    row: &mut ProductPageView,
) -> anyhow::Result<()> {
    row.category_id = Some(event.data.category_id);
    Ok(())
}
