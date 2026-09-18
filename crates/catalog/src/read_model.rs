//! SQL list read model: one row per product, for brand/category listings.
//! Fed by the `catalog-product-list` subscription; the page itself is served
//! by the [`crate::ProductPageView`] projection.

use evento::{
    Executor,
    metadata::Event,
    subscription::{Context, SubscriptionBuilder},
};
use sqlx::SqlitePool;

use crate::aggregator::{
    ProductArchived, ProductCreated, ProductDescribed, ProductEnergyLabelled, ProductMediaAdded,
    ProductSpecified,
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
}

pub fn product_list_subscription<E: Executor>() -> SubscriptionBuilder<E> {
    SubscriptionBuilder::new(PRODUCT_LIST_SUBSCRIPTION)
        .handler(insert_on_product_created())
        .handler(flag_on_product_archived())
        .skip::<ProductDescribed>()
        .skip::<ProductSpecified>()
        .skip::<ProductMediaAdded>()
        .skip::<ProductEnergyLabelled>()
        .strict()
}

pub async fn list_by_brand(db: &SqlitePool, brand_slug: &str) -> sqlx::Result<Vec<ProductListRow>> {
    sqlx::query_as(
        "SELECT id, sku, name, brand_slug, category_path, archived
         FROM catalog_product
         WHERE brand_slug = ? AND archived = 0
         ORDER BY name",
    )
    .bind(brand_slug)
    .fetch_all(db)
    .await
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
        "SELECT id, sku, name, brand_slug, category_path, archived
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

#[evento::subscription]
async fn insert_on_product_created<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<ProductCreated>,
) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT OR IGNORE INTO catalog_product (id, sku, name, brand_slug, category_path)
         VALUES (?, ?, ?, ?, ?)",
    )
    .bind(&event.aggregate_id)
    .bind(&event.data.sku)
    .bind(&event.data.name)
    .bind(&event.data.brand.slug)
    .bind(event.data.category_path.join(" > "))
    .execute(&pool(ctx)?)
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
