//! Products in the store's catalog.
//!
//! Every product enters the catalog through [`import_product`] — sourced from
//! a provider, including `self_inventory` for products the store creates by
//! hand. Stock lives in the separate co-keyed [`crate::inventory`] aggregate
//! so stock churn never contends with the product's version.

use anyhow::Result;
use evento::ProjectionAggregate;
use evento::metadata::Event;
use evento::projection::Projection;
use evento::sql::RwSqlite;
use timada_provider::SourceProduct;

/// A purchasable variant, as captured at import time.
#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    Default,
    bitcode::Encode,
    bitcode::Decode,
    serde::Serialize,
    serde::Deserialize,
)]
pub struct ImportedVariant {
    pub external_ref: String,
    pub title: String,
    pub price_amount_minor: i64,
    pub currency: String,
    pub options: Vec<(String, String)>,
}

#[evento::aggregate]
pub enum Product {
    /// The product entered the catalog, sourced from a provider
    /// (`self_inventory` for manually created products).
    ProductImported {
        provider_kind: String,
        connection_id: String,
        external_ref: String,
        title: String,
        description: String,
        image_urls: Vec<String>,
        price_amount_minor: i64,
        currency: String,
        variants: Vec<ImportedVariant>,
    },
    ProductDetailsRevised {
        title: String,
        description: String,
    },
    ProductRepriced {
        amount_minor: i64,
        currency: String,
    },
    ProductPublished,
    ProductUnpublished,
    ProductArchived,
}

/// Write-side state, replayed from the product's events.
#[evento::projection(bitcode::Encode, bitcode::Decode)]
#[derive(Debug)]
pub struct ProductState {
    pub id: String,
    pub provider_kind: String,
    pub connection_id: String,
    pub external_ref: String,
    pub title: String,
    pub description: String,
    pub price_amount_minor: i64,
    pub currency: String,
    pub published: bool,
    pub archived: bool,
}

impl ProjectionAggregate for ProductState {
    fn aggregate_id(&self) -> String {
        self.id.to_owned()
    }
}

#[evento::handler]
async fn on_product_imported(
    event: Event<ProductImported>,
    state: &mut ProductState,
) -> Result<()> {
    state.id = event.aggregate_id.clone();
    state.provider_kind = event.data.provider_kind.clone();
    state.connection_id = event.data.connection_id.clone();
    state.external_ref = event.data.external_ref.clone();
    state.title = event.data.title.clone();
    state.description = event.data.description.clone();
    state.price_amount_minor = event.data.price_amount_minor;
    state.currency = event.data.currency.clone();
    Ok(())
}

#[evento::handler]
async fn on_product_details_revised(
    event: Event<ProductDetailsRevised>,
    state: &mut ProductState,
) -> Result<()> {
    state.title = event.data.title.clone();
    state.description = event.data.description.clone();
    Ok(())
}

#[evento::handler]
async fn on_product_repriced(
    event: Event<ProductRepriced>,
    state: &mut ProductState,
) -> Result<()> {
    state.price_amount_minor = event.data.amount_minor;
    state.currency = event.data.currency.clone();
    Ok(())
}

#[evento::handler]
async fn on_product_published(
    _event: Event<ProductPublished>,
    state: &mut ProductState,
) -> Result<()> {
    state.published = true;
    Ok(())
}

#[evento::handler]
async fn on_product_unpublished(
    _event: Event<ProductUnpublished>,
    state: &mut ProductState,
) -> Result<()> {
    state.published = false;
    Ok(())
}

#[evento::handler]
async fn on_product_archived(
    _event: Event<ProductArchived>,
    state: &mut ProductState,
) -> Result<()> {
    state.archived = true;
    state.published = false;
    Ok(())
}

fn state_projection() -> Projection<RwSqlite, ProductState> {
    Projection::new::<Product>()
        .handler(on_product_imported())
        .handler(on_product_details_revised())
        .handler(on_product_repriced())
        .handler(on_product_published())
        .handler(on_product_unpublished())
        .handler(on_product_archived())
        .strict()
}

/// Load the write-side state of one product.
pub async fn load(executor: &RwSqlite, id: &str) -> Result<Option<ProductState>> {
    state_projection().load(id).execute(executor).await
}

/// Bring a provider-sourced product into the catalog. Returns the new
/// product's id. The product starts unpublished.
#[tracing::instrument(skip_all, fields(provider_kind, external_ref = %source.external_ref))]
pub async fn import_product(
    executor: &RwSqlite,
    source: SourceProduct,
    provider_kind: &str,
    connection_id: &str,
) -> Result<String> {
    if source.title.trim().is_empty() {
        anyhow::bail!("a product needs a title");
    }
    if source.price.amount_minor <= 0 {
        anyhow::bail!("a product's price must be positive");
    }

    let variants = source
        .variants
        .into_iter()
        .map(|variant| ImportedVariant {
            external_ref: variant.external_ref,
            title: variant.title,
            price_amount_minor: variant.price.amount_minor,
            currency: variant.price.currency,
            options: variant.options,
        })
        .collect();

    let id = evento::create()
        .event(&ProductImported {
            provider_kind: provider_kind.to_owned(),
            connection_id: connection_id.to_owned(),
            external_ref: source.external_ref,
            title: source.title,
            description: source.description,
            image_urls: source.image_urls,
            price_amount_minor: source.price.amount_minor,
            currency: source.price.currency,
            variants,
        })
        .commit(executor)
        .await?;

    tracing::info!(aggregate_id = %id, "product imported");
    Ok(id)
}

/// Change the product's title and description.
#[tracing::instrument(skip_all, fields(aggregate_id = %id))]
pub async fn revise_product_details(
    executor: &RwSqlite,
    id: &str,
    title: String,
    description: String,
) -> Result<()> {
    if title.trim().is_empty() {
        anyhow::bail!("a product needs a title");
    }
    let Some(state) = load(executor, id).await? else {
        anyhow::bail!("product not found: {id}");
    };
    if state.archived {
        anyhow::bail!("an archived product cannot be revised");
    }

    state
        .write()?
        .event(&ProductDetailsRevised { title, description })
        .commit(executor)
        .await?;

    tracing::info!("product details revised");
    Ok(())
}

/// Change the product's price.
#[tracing::instrument(skip_all, fields(aggregate_id = %id))]
pub async fn reprice_product(
    executor: &RwSqlite,
    id: &str,
    amount_minor: i64,
    currency: String,
) -> Result<()> {
    if amount_minor <= 0 {
        anyhow::bail!("a product's price must be positive");
    }
    let Some(state) = load(executor, id).await? else {
        anyhow::bail!("product not found: {id}");
    };
    if state.archived {
        anyhow::bail!("an archived product cannot be repriced");
    }

    state
        .write()?
        .event(&ProductRepriced {
            amount_minor,
            currency,
        })
        .commit(executor)
        .await?;

    tracing::info!("product repriced");
    Ok(())
}

/// Make the product visible on the storefront.
#[tracing::instrument(skip_all, fields(aggregate_id = %id))]
pub async fn publish_product(executor: &RwSqlite, id: &str) -> Result<()> {
    let Some(state) = load(executor, id).await? else {
        anyhow::bail!("product not found: {id}");
    };
    if state.archived {
        anyhow::bail!("an archived product cannot be published");
    }
    if state.published {
        anyhow::bail!("product is already published");
    }

    state
        .write()?
        .event(&ProductPublished)
        .commit(executor)
        .await?;

    tracing::info!("product published");
    Ok(())
}

/// Take the product off the storefront without archiving it.
#[tracing::instrument(skip_all, fields(aggregate_id = %id))]
pub async fn unpublish_product(executor: &RwSqlite, id: &str) -> Result<()> {
    let Some(state) = load(executor, id).await? else {
        anyhow::bail!("product not found: {id}");
    };
    if !state.published {
        anyhow::bail!("product is not published");
    }

    state
        .write()?
        .event(&ProductUnpublished)
        .commit(executor)
        .await?;

    tracing::info!("product unpublished");
    Ok(())
}

/// Retire the product permanently. Archived products cannot be revised,
/// repriced, or published again.
#[tracing::instrument(skip_all, fields(aggregate_id = %id))]
pub async fn archive_product(executor: &RwSqlite, id: &str) -> Result<()> {
    let Some(state) = load(executor, id).await? else {
        anyhow::bail!("product not found: {id}");
    };
    if state.archived {
        anyhow::bail!("product is already archived");
    }

    state
        .write()?
        .event(&ProductArchived)
        .commit(executor)
        .await?;

    tracing::info!("product archived");
    Ok(())
}
