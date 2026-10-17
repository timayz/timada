//! Loads a snapshot into a catalog through its commands.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use evento::Executor;
use timada_catalog::{
    AddImage, CatalogError, Command, CreateCategory, CreateProduct, DefineVariant, OptionSelection,
    ProductState,
};

use crate::snapshot::{CategorySeed, ProductSeed, Snapshot};

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// A picture the host stored, as the catalog records it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredImage {
    pub image_id: String,
    /// Lowercase, no dot.
    pub ext: String,
    pub size: u64,
    pub width: Option<u32>,
    pub height: Option<u32>,
}

/// Where a product's pictures come from and go: the host fetches the bytes
/// at a URL and keeps them where its catalog serves pictures from. `None`
/// means the picture is left out (not found, not a picture, too large);
/// the import goes on without it.
pub trait Images: Send + Sync {
    fn fetch<'a>(&'a self, url: &'a str) -> BoxFuture<'a, anyhow::Result<Option<StoredImage>>>;

    /// Drops a picture the catalog ended up not recording.
    fn discard<'a>(&'a self, image: &'a StoredImage) -> BoxFuture<'a, ()> {
        let _ = image;
        Box::pin(async {})
    }
}

/// Imports without pictures.
pub struct NoImages;

impl Images for NoImages {
    fn fetch<'a>(&'a self, _url: &'a str) -> BoxFuture<'a, anyhow::Result<Option<StoredImage>>> {
        Box::pin(async { Ok(None) })
    }
}

#[derive(Debug, Clone)]
pub struct ImportOptions {
    /// At most this many products (categories are always all loaded).
    pub limit: Option<usize>,
    /// Pictures fetched at once.
    pub concurrency: usize,
}

impl Default for ImportOptions {
    fn default() -> Self {
        Self {
            limit: None,
            concurrency: 8,
        }
    }
}

/// What a run did.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Report {
    pub categories_created: usize,
    /// Already there, by slug.
    pub categories_skipped: usize,
    pub products_created: usize,
    /// Already there, by slug.
    pub products_skipped: usize,
    pub variants: usize,
    pub images: usize,
    /// What went wrong for one product or category without stopping the run.
    pub warnings: Vec<String>,
}

/// Loads `snapshot` into the catalog as `actor`. Categories first, parents
/// before children; then the products, each created, filled in and
/// published, or taken off again when something refused it, so the next
/// run can try again. Whatever is already there by slug is skipped.
pub async fn import<E: Executor>(
    catalog: &Command<E>,
    actor: &str,
    snapshot: &Snapshot,
    images: Arc<dyn Images>,
    options: &ImportOptions,
) -> anyhow::Result<Report> {
    let mut report = Report::default();
    let categories = import_categories(catalog, actor, &snapshot.categories, &mut report).await?;

    let total = options.limit.map_or(snapshot.products.len(), |limit| {
        limit.min(snapshot.products.len())
    });
    for (index, seed) in snapshot.products.iter().take(total).enumerate() {
        if catalog.load_product_by_slug(&seed.slug).await?.is_some() {
            report.products_skipped += 1;
        } else {
            match import_product(catalog, actor, seed, &categories, images.clone(), options).await {
                Ok(outcome) => {
                    report.products_created += 1;
                    report.variants += outcome.variants;
                    report.images += outcome.images;
                    report.warnings.extend(outcome.warnings);
                }
                Err(err) => {
                    tracing::warn!(slug = %seed.slug, %err, "product not imported");
                    report.warnings.push(format!("{}: {err:#}", seed.slug));
                }
            }
        }
        if (index + 1) % 50 == 0 || index + 1 == total {
            tracing::info!(
                done = index + 1,
                total,
                created = report.products_created,
                skipped = report.products_skipped,
                "products"
            );
        }
    }
    Ok(report)
}

/// Creates the categories that are missing, parents first, and returns
/// every category's id by slug.
async fn import_categories<E: Executor>(
    catalog: &Command<E>,
    actor: &str,
    seeds: &[CategorySeed],
    report: &mut Report,
) -> anyhow::Result<HashMap<String, String>> {
    let by_slug: HashMap<&str, &CategorySeed> =
        seeds.iter().map(|c| (c.slug.as_str(), c)).collect();
    let depth = |seed: &CategorySeed| {
        let mut depth = 0;
        let mut parent = seed.parent.as_deref();
        while let Some(slug) = parent {
            depth += 1;
            parent = by_slug.get(slug).and_then(|p| p.parent.as_deref());
            if depth > seeds.len() {
                break;
            }
        }
        depth
    };
    let mut ordered: Vec<&CategorySeed> = seeds.iter().collect();
    ordered.sort_by_key(|seed| depth(seed));

    let mut ids: HashMap<String, String> = HashMap::new();
    for seed in ordered {
        let parent_id = match &seed.parent {
            Some(parent) => match ids.get(parent) {
                Some(id) => Some(id.clone()),
                None => {
                    report
                        .warnings
                        .push(format!("category {}: parent {parent} missing", seed.slug));
                    continue;
                }
            },
            None => None,
        };
        if let Some(existing) = catalog.load_category_by_slug(&seed.slug).await? {
            ids.insert(seed.slug.clone(), existing.id);
            report.categories_skipped += 1;
            continue;
        }
        let created = match catalog
            .create_category(
                actor,
                CreateCategory {
                    name: seed.name.clone(),
                    slug: Some(seed.slug.clone()),
                    parent_id,
                },
            )
            .await
        {
            Ok(created) => created,
            Err(err) => {
                tracing::warn!(slug = %seed.slug, %err, "category not created");
                report
                    .warnings
                    .push(format!("category {}: {err}", seed.slug));
                continue;
            }
        };
        if !seed.description.is_empty() {
            catalog
                .describe_category(actor, &created.id, &seed.description)
                .await?;
        }
        if seed.position != 0 {
            catalog
                .position_category(actor, &created.id, seed.position)
                .await?;
        }
        if !seed.in_menu {
            catalog
                .set_category_in_menu(actor, &created.id, false)
                .await?;
        }
        ids.insert(seed.slug.clone(), created.id);
        report.categories_created += 1;
    }
    Ok(ids)
}

#[derive(Default)]
struct ProductOutcome {
    variants: usize,
    images: usize,
    warnings: Vec<String>,
}

async fn import_product<E: Executor>(
    catalog: &Command<E>,
    actor: &str,
    seed: &ProductSeed,
    categories: &HashMap<String, String>,
    images: Arc<dyn Images>,
    options: &ImportOptions,
) -> anyhow::Result<ProductOutcome> {
    let product = match catalog
        .create_product(
            actor,
            CreateProduct {
                name: seed.name.clone(),
                slug: Some(seed.slug.clone()),
            },
        )
        .await
    {
        Ok(product) => product,
        // A slug the catalog refuses as given: let it derive one instead.
        Err(CatalogError::SlugTaken | CatalogError::InvalidSlug) => {
            tracing::warn!(slug = %seed.slug, "slug refused, one derived from the name instead");
            catalog
                .create_product(
                    actor,
                    CreateProduct {
                        name: seed.name.clone(),
                        slug: None,
                    },
                )
                .await?
        }
        Err(err) => return Err(err.into()),
    };
    match fill_product(catalog, actor, &product, seed, categories, images, options).await {
        Ok(outcome) => Ok(outcome),
        Err(err) => {
            // Half a product is worse than none: taken off, which frees the
            // slug for the next run.
            if let Err(archive) = catalog.archive_product(actor, &product.id).await {
                tracing::warn!(slug = %product.slug, %archive, "cannot take the failed product off");
            }
            Err(err)
        }
    }
}

async fn fill_product<E: Executor>(
    catalog: &Command<E>,
    actor: &str,
    product: &ProductState,
    seed: &ProductSeed,
    categories: &HashMap<String, String>,
    images: Arc<dyn Images>,
    options: &ImportOptions,
) -> anyhow::Result<ProductOutcome> {
    let id = product.id.as_str();
    let mut outcome = ProductOutcome::default();
    if !seed.description.is_empty() {
        catalog
            .describe_product(actor, id, &seed.description)
            .await?;
    }
    for attribute in &seed.attributes {
        if let Err(err) = catalog
            .set_attribute(actor, id, &attribute.name, &attribute.value)
            .await
        {
            outcome
                .warnings
                .push(format!("{}: fact {}: {err}", seed.slug, attribute.name));
        }
    }
    for slug in &seed.categories {
        match categories.get(slug) {
            Some(category_id) => catalog.categorize_product(actor, id, category_id).await?,
            None => outcome
                .warnings
                .push(format!("{}: category {slug} unknown", seed.slug)),
        }
    }

    // The pictures, fetched a few at a time, recorded in the snapshot's
    // order. `image_ids[i]` is the id of `seed.images[i]` once recorded.
    let mut image_ids: Vec<Option<String>> = vec![None; seed.images.len()];
    let mut fetched: Vec<Option<StoredImage>> = Vec::with_capacity(seed.images.len());
    for chunk in seed.images.chunks(options.concurrency.max(1)) {
        let mut set = tokio::task::JoinSet::new();
        for (offset, image) in chunk.iter().enumerate() {
            let images = images.clone();
            let url = image.url.clone();
            set.spawn(async move { (offset, images.fetch(&url).await) });
        }
        let mut results: Vec<Option<StoredImage>> = vec![None; chunk.len()];
        while let Some(joined) = set.join_next().await {
            let (offset, result) = joined?;
            match result {
                Ok(stored) => results[offset] = stored,
                Err(err) => {
                    tracing::warn!(url = %chunk[offset].url, %err, "picture not fetched");
                    outcome.warnings.push(format!(
                        "{}: picture {}: {err:#}",
                        seed.slug, chunk[offset].url
                    ));
                }
            }
        }
        fetched.extend(results);
    }
    for (index, stored) in fetched.into_iter().enumerate() {
        let Some(stored) = stored else { continue };
        let added = catalog
            .add_image(
                actor,
                id,
                AddImage {
                    image_id: stored.image_id.clone(),
                    ext: stored.ext.clone(),
                    size: stored.size,
                    width: stored.width,
                    height: stored.height,
                    alt: seed.images[index].alt.clone(),
                },
            )
            .await;
        match added {
            Ok(()) => {
                image_ids[index] = Some(stored.image_id);
                outcome.images += 1;
            }
            Err(err) => {
                images.discard(&stored).await;
                outcome.warnings.push(format!(
                    "{}: picture {}: {err}",
                    seed.slug, seed.images[index].url
                ));
            }
        }
    }

    if seed.variants.is_empty() {
        let amount = seed
            .amount_cents
            .ok_or_else(|| anyhow::anyhow!("neither a price nor variants"))?;
        catalog
            .price_product(actor, id, amount, &seed.currency)
            .await?;
    } else {
        let mut option_ids: HashMap<&str, String> = HashMap::new();
        for option in &seed.options {
            let option_id = catalog
                .define_option(actor, id, &option.name, option.values.clone())
                .await?;
            option_ids.insert(option.name.as_str(), option_id);
        }
        for variant in &seed.variants {
            let mut selections = Vec::with_capacity(variant.selections.len());
            for selection in &variant.selections {
                let Some(option_id) = option_ids.get(selection.option.as_str()) else {
                    anyhow::bail!(
                        "variant names an option the product lacks: {}",
                        selection.option
                    );
                };
                selections.push(OptionSelection {
                    option_id: option_id.clone(),
                    value: selection.value.clone(),
                });
            }
            let image_id = variant
                .image
                .and_then(|index| image_ids.get(index).cloned().flatten());
            let define = |sku: Option<String>| DefineVariant {
                selections: selections.clone(),
                sku,
                amount_cents: variant.amount_cents,
                currency: seed.currency.clone(),
                image_id: image_id.clone(),
            };
            let defined = match catalog
                .define_variant(actor, id, define(variant.sku.clone()))
                .await
            {
                Err(CatalogError::SkuTaken | CatalogError::InvalidSku) if variant.sku.is_some() => {
                    outcome.warnings.push(format!(
                        "{}: code {} refused, variant kept without one",
                        seed.slug,
                        variant.sku.as_deref().unwrap_or_default()
                    ));
                    catalog.define_variant(actor, id, define(None)).await
                }
                other => other,
            };
            match defined {
                Ok(_) => outcome.variants += 1,
                Err(CatalogError::VariantExists) => {}
                Err(err) => {
                    outcome.warnings.push(format!(
                        "{}: variant {:?}: {err}",
                        seed.slug, variant.selections
                    ));
                }
            }
        }
    }
    catalog.publish_product(actor, id).await?;
    Ok(outcome)
}
