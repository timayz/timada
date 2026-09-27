//! SQL list read model: one row per product, for brand/category listings —
//! and for the product's storefront address, whose last segment (its `slug`)
//! is kept here. Fed by the `catalog-product-list` subscription; the page
//! itself is served by the [`crate::ProductPageView`] projection.

use std::collections::HashMap;

use evento::{
    Executor,
    metadata::Event,
    subscription::{Context, SubscriptionBuilder},
};
use sqlx::SqlitePool;
use timada_core::slug::slugify;

use crate::{
    aggregator::{
        ProductArchived, ProductCategorised, ProductCreated, ProductDescribed,
        ProductEnergyLabelled, ProductJoinedFamily, ProductLeftFamily, ProductMediaAdded,
        ProductSpecified,
    },
    command::MAX_CATEGORY_DEPTH,
    list_categories,
};

/// Subscription key; the caller attaches the pool with `.data(pool)`.
pub const PRODUCT_LIST_SUBSCRIPTION: &str = "catalog-product-list";

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct ProductListRow {
    pub id: String,
    pub sku: String,
    pub name: String,
    pub brand_slug: String,
    pub category_path: String,
    pub archived: bool,
    /// The category the product is filed under, once it has been.
    pub category_id: Option<String>,
    /// The last segment of the product's storefront address: its name as a
    /// slug, the SKU appended when another product already spells the same
    /// — unique, and for ever, since a product is never renamed. Empty only
    /// for a row from before slugs, until [`fill_product_slugs`] runs.
    pub slug: String,
}

pub fn product_list_subscription<E: Executor>() -> SubscriptionBuilder<E> {
    SubscriptionBuilder::new(PRODUCT_LIST_SUBSCRIPTION)
        .handler(insert_on_product_created())
        .handler(flag_on_product_archived())
        .handler(file_on_product_categorised())
        .skip::<ProductDescribed>()
        .skip::<ProductSpecified>()
        .skip::<ProductMediaAdded>()
        .skip::<ProductEnergyLabelled>()
        .skip::<ProductJoinedFamily>()
        .skip::<ProductLeftFamily>()
        .strict()
}

pub async fn list_by_brand(db: &SqlitePool, brand_slug: &str) -> sqlx::Result<Vec<ProductListRow>> {
    sqlx::query_as(
        "SELECT id, sku, name, brand_slug, category_path, archived, category_id, slug
         FROM catalog_product
         WHERE brand_slug = ? AND archived = 0
         ORDER BY name",
    )
    .bind(brand_slug)
    .fetch_all(db)
    .await
}

/// The list rows of the given products, in no particular order — for pages
/// of another context (stock levels, order lines) that need names and SKUs.
pub async fn products_by_ids(db: &SqlitePool, ids: &[String]) -> sqlx::Result<Vec<ProductListRow>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let mut query = sqlx::QueryBuilder::<sqlx::Sqlite>::new(
        "SELECT id, sku, name, brand_slug, category_path, archived, category_id, slug
         FROM catalog_product
         WHERE id IN (",
    );
    let mut bound = query.separated(", ");
    for id in ids {
        bound.push_bind(id);
    }
    query.push(")");
    query.build_query_as().fetch_all(db).await
}

/// The product at a storefront address's last segment, archived or not.
pub async fn product_by_slug(db: &SqlitePool, slug: &str) -> sqlx::Result<Option<ProductListRow>> {
    if slug.is_empty() {
        return Ok(None);
    }
    sqlx::query_as(
        "SELECT id, sku, name, brand_slug, category_path, archived, category_id, slug
         FROM catalog_product
         WHERE slug = ?",
    )
    .bind(slug)
    .fetch_optional(db)
    .await
}

/// The storefront address of each of `ids`, as its segments: the slugs of
/// the way down to the product's category — archived or not: the address
/// does not move when the category leaves the storefront — then the
/// product's own. A product without a category is at its slug alone.
/// Products the list does not know, or without a slug yet, are left out.
pub async fn storefront_paths(
    db: &SqlitePool,
    ids: &[String],
) -> sqlx::Result<HashMap<String, Vec<String>>> {
    let products = products_by_ids(db, ids).await?;
    if products.is_empty() {
        return Ok(HashMap::new());
    }
    let categories: HashMap<String, (String, Option<String>)> = list_categories(db, true)
        .await?
        .into_iter()
        .map(|c| (c.id, (c.slug, c.parent_id)))
        .collect();
    Ok(products
        .into_iter()
        .filter(|product| !product.slug.is_empty())
        .map(|product| {
            let mut segments = Vec::new();
            let mut next = product.category_id.clone();
            while let Some(id) = next {
                let Some((slug, parent_id)) = categories.get(&id) else {
                    break;
                };
                if segments.len() >= MAX_CATEGORY_DEPTH || segments.contains(slug) {
                    break;
                }
                segments.push(slug.clone());
                next = parent_id.clone();
            }
            segments.reverse();
            segments.push(product.slug.clone());
            (product.id, segments)
        })
        .collect())
}

/// The slug a product goes by: its name, or — when another product already
/// spells the same, or the name has no letter or digit — its name and SKU.
async fn free_slug(db: &SqlitePool, id: &str, name: &str, sku: &str) -> sqlx::Result<String> {
    let plain = slugify(name);
    let with_sku = if plain.is_empty() {
        slugify(sku)
    } else {
        format!("{plain}-{}", slugify(sku))
    };
    for candidate in [plain, with_sku] {
        if candidate.is_empty() {
            continue;
        }
        let taken_by: Option<String> =
            sqlx::query_scalar("SELECT id FROM catalog_product WHERE slug = ?")
                .bind(&candidate)
                .fetch_optional(db)
                .await?;
        if taken_by.is_none_or(|other| other == id) {
            return Ok(candidate);
        }
    }
    // A SKU is unique, so this is a name and SKU without a single letter or
    // digit between them: the id, which derives from the SKU, will do.
    Ok(id.to_owned())
}

/// Gives the rows from before slugs theirs — the same ones the subscription
/// would have given them. Called once at start-up; a no-op afterwards.
pub async fn fill_product_slugs(db: &SqlitePool) -> sqlx::Result<u64> {
    let rows: Vec<(String, String, String)> =
        sqlx::query_as("SELECT id, name, sku FROM catalog_product WHERE slug = '' ORDER BY rowid")
            .fetch_all(db)
            .await?;
    let mut filled = 0;
    for (id, name, sku) in rows {
        let slug = free_slug(db, &id, &name, &sku).await?;
        filled += sqlx::query("UPDATE catalog_product SET slug = ? WHERE id = ?")
            .bind(slug)
            .bind(id)
            .execute(db)
            .await?
            .rows_affected();
    }
    Ok(filled)
}

/// Admin listing: free-text search on name or SKU, archived products hidden
/// unless asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListProducts {
    pub q: Option<String>,
    pub include_archived: bool,
    pub limit: u32,
    pub offset: u32,
}

impl Default for ListProducts {
    fn default() -> Self {
        Self {
            q: None,
            include_archived: false,
            limit: 50,
            offset: 0,
        }
    }
}

fn like_pattern(q: Option<&str>) -> Option<String> {
    q.map(str::trim)
        .filter(|q| !q.is_empty())
        .map(|q| format!("%{q}%"))
}

pub async fn list_products(
    db: &SqlitePool,
    query: &ListProducts,
) -> sqlx::Result<Vec<ProductListRow>> {
    sqlx::query_as(
        "SELECT id, sku, name, brand_slug, category_path, archived, category_id, slug
         FROM catalog_product
         WHERE (?1 IS NULL OR name LIKE ?1 OR sku LIKE ?1)
           AND (?2 OR archived = 0)
         ORDER BY name
         LIMIT ?3 OFFSET ?4",
    )
    .bind(like_pattern(query.q.as_deref()))
    .bind(query.include_archived)
    .bind(query.limit)
    .bind(query.offset)
    .fetch_all(db)
    .await
}

pub async fn count_products(
    db: &SqlitePool,
    q: Option<&str>,
    include_archived: bool,
) -> sqlx::Result<i64> {
    sqlx::query_scalar(
        "SELECT COUNT(*) FROM catalog_product
         WHERE (?1 IS NULL OR name LIKE ?1 OR sku LIKE ?1)
           AND (?2 OR archived = 0)",
    )
    .bind(like_pattern(q))
    .bind(include_archived)
    .fetch_one(db)
    .await
}

fn pool<E: Executor>(ctx: &Context<'_, E>) -> anyhow::Result<SqlitePool> {
    ctx.get::<SqlitePool>()
        .ok_or_else(|| anyhow::anyhow!("SqlitePool missing from subscription context"))
}

/// A redelivery finds the row there and leaves it — its slug included.
#[evento::subscription]
async fn insert_on_product_created<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<ProductCreated>,
) -> anyhow::Result<()> {
    let db = pool(ctx)?;
    let slug = free_slug(&db, &event.aggregate_id, &event.data.name, &event.data.sku).await?;
    sqlx::query(
        "INSERT OR IGNORE INTO catalog_product (id, sku, name, brand_slug, category_path, slug)
         VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(&event.aggregate_id)
    .bind(&event.data.sku)
    .bind(&event.data.name)
    .bind(&event.data.brand.slug)
    .bind(event.data.category_path.join(" > "))
    .bind(slug)
    .execute(&db)
    .await?;
    Ok(())
}

#[evento::subscription]
async fn flag_on_product_archived<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<ProductArchived>,
) -> anyhow::Result<()> {
    sqlx::query("UPDATE catalog_product SET archived = 1 WHERE id = ?")
        .bind(&event.aggregate_id)
        .execute(&pool(ctx)?)
        .await?;
    Ok(())
}

/// The last filing wins; a redelivery files the product where it already is.
#[evento::subscription]
async fn file_on_product_categorised<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<ProductCategorised>,
) -> anyhow::Result<()> {
    sqlx::query("UPDATE catalog_product SET category_id = ? WHERE id = ?")
        .bind(&event.data.category_id)
        .bind(&event.aggregate_id)
        .execute(&pool(ctx)?)
        .await?;
    Ok(())
}
