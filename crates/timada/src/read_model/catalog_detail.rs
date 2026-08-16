//! Read model for product detail pages (storefront product page, and any
//! consumer that needs the full imported payload: images, variants,
//! provenance). One denormalized row per product; lists are JSON columns.

use anyhow::Result;
use evento::Executor;
use evento::metadata::Event;
use evento::sql::RwSqlite;
use evento::subscription::{Context, Subscription, SubscriptionBuilder};
use sqlx::SqlitePool;
use sqlx_migrator::{sqlite_migration, vec_box};

use crate::inventory::StockAdjusted;
use crate::product::{
    ImportedVariant, ProductArchived, ProductDetailsRevised, ProductImported, ProductPublished,
    ProductRepriced, ProductUnpublished,
};

pub struct M0003CreateCatalogDetail;

sqlite_migration!(
    M0003CreateCatalogDetail,
    "timada",
    "m0003_create_catalog_detail",
    vec_box![],
    vec_box![(
        "CREATE TABLE catalog_detail (
            id TEXT PRIMARY KEY,
            title TEXT NOT NULL,
            description TEXT NOT NULL DEFAULT '',
            image_urls TEXT NOT NULL DEFAULT '[]',
            amount_minor INTEGER NOT NULL,
            currency TEXT NOT NULL,
            status TEXT NOT NULL DEFAULT 'draft',
            provider_kind TEXT NOT NULL,
            connection_id TEXT NOT NULL DEFAULT '',
            external_ref TEXT NOT NULL DEFAULT '',
            variants TEXT NOT NULL DEFAULT '[]',
            stock_available INTEGER NOT NULL DEFAULT 0
        )",
        "DROP TABLE catalog_detail"
    )]
);

#[derive(Debug, Clone, sqlx::FromRow)]
struct CatalogDetailRow {
    id: String,
    title: String,
    description: String,
    image_urls: String,
    amount_minor: i64,
    currency: String,
    status: String,
    provider_kind: String,
    connection_id: String,
    external_ref: String,
    variants: String,
    stock_available: i64,
}

/// A product detail with JSON columns parsed.
#[derive(Debug, Clone)]
pub struct CatalogDetail {
    pub id: String,
    pub title: String,
    pub description: String,
    pub image_urls: Vec<String>,
    pub amount_minor: i64,
    pub currency: String,
    pub status: String,
    pub provider_kind: String,
    pub connection_id: String,
    pub external_ref: String,
    pub variants: Vec<ImportedVariant>,
    pub stock_available: i64,
}

impl TryFrom<CatalogDetailRow> for CatalogDetail {
    type Error = anyhow::Error;

    fn try_from(row: CatalogDetailRow) -> Result<Self> {
        Ok(Self {
            image_urls: serde_json::from_str(&row.image_urls)?,
            variants: serde_json::from_str(&row.variants)?,
            id: row.id,
            title: row.title,
            description: row.description,
            amount_minor: row.amount_minor,
            currency: row.currency,
            status: row.status,
            provider_kind: row.provider_kind,
            connection_id: row.connection_id,
            external_ref: row.external_ref,
            stock_available: row.stock_available,
        })
    }
}

pub async fn by_id(db: &SqlitePool, id: &str) -> Result<Option<CatalogDetail>> {
    let row: Option<CatalogDetailRow> = sqlx::query_as(
        "SELECT id, title, description, image_urls, amount_minor, currency, status,
                provider_kind, connection_id, external_ref, variants, stock_available
         FROM catalog_detail WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(db)
    .await?;

    row.map(CatalogDetail::try_from).transpose()
}

/// Self-inventory products matching a title search — backs the
/// `self_inventory` provider's catalog sourcing.
pub async fn search_self_inventory(
    db: &SqlitePool,
    search: &str,
    limit: u16,
) -> Result<Vec<CatalogDetail>> {
    let rows: Vec<CatalogDetailRow> = sqlx::query_as(
        "SELECT id, title, description, image_urls, amount_minor, currency, status,
                provider_kind, connection_id, external_ref, variants, stock_available
         FROM catalog_detail
         WHERE provider_kind = 'self_inventory' AND title LIKE ?
         ORDER BY id DESC LIMIT ?",
    )
    .bind(format!("%{}%", search.trim()))
    .bind(limit)
    .fetch_all(db)
    .await?;

    rows.into_iter().map(CatalogDetail::try_from).collect()
}

fn db<E: Executor>(ctx: &Context<'_, E>) -> Result<SqlitePool> {
    ctx.get::<SqlitePool>()
        .ok_or_else(|| anyhow::anyhow!("SqlitePool not injected into subscription"))
}

#[evento::subscription]
async fn on_product_imported<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<ProductImported>,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO catalog_detail
            (id, title, description, image_urls, amount_minor, currency,
             provider_kind, connection_id, external_ref, variants)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?) ON CONFLICT(id) DO NOTHING",
    )
    .bind(&event.aggregate_id)
    .bind(&event.data.title)
    .bind(&event.data.description)
    .bind(serde_json::to_string(&event.data.image_urls)?)
    .bind(event.data.price_amount_minor)
    .bind(&event.data.currency)
    .bind(&event.data.provider_kind)
    .bind(&event.data.connection_id)
    .bind(&event.data.external_ref)
    .bind(serde_json::to_string(&event.data.variants)?)
    .execute(&db(ctx)?)
    .await?;
    Ok(())
}

#[evento::subscription]
async fn on_product_details_revised<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<ProductDetailsRevised>,
) -> Result<()> {
    sqlx::query("UPDATE catalog_detail SET title = ?, description = ? WHERE id = ?")
        .bind(&event.data.title)
        .bind(&event.data.description)
        .bind(&event.aggregate_id)
        .execute(&db(ctx)?)
        .await?;
    Ok(())
}

#[evento::subscription]
async fn on_product_repriced<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<ProductRepriced>,
) -> Result<()> {
    sqlx::query("UPDATE catalog_detail SET amount_minor = ?, currency = ? WHERE id = ?")
        .bind(event.data.amount_minor)
        .bind(&event.data.currency)
        .bind(&event.aggregate_id)
        .execute(&db(ctx)?)
        .await?;
    Ok(())
}

#[evento::subscription]
async fn on_product_published<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<ProductPublished>,
) -> Result<()> {
    sqlx::query("UPDATE catalog_detail SET status = 'published' WHERE id = ?")
        .bind(&event.aggregate_id)
        .execute(&db(ctx)?)
        .await?;
    Ok(())
}

#[evento::subscription]
async fn on_product_unpublished<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<ProductUnpublished>,
) -> Result<()> {
    sqlx::query("UPDATE catalog_detail SET status = 'draft' WHERE id = ?")
        .bind(&event.aggregate_id)
        .execute(&db(ctx)?)
        .await?;
    Ok(())
}

#[evento::subscription]
async fn on_product_archived<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<ProductArchived>,
) -> Result<()> {
    sqlx::query("UPDATE catalog_detail SET status = 'archived' WHERE id = ?")
        .bind(&event.aggregate_id)
        .execute(&db(ctx)?)
        .await?;
    Ok(())
}

#[evento::subscription]
async fn on_stock_adjusted<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<StockAdjusted>,
) -> Result<()> {
    // Absolute value from the event, so replays are idempotent.
    sqlx::query("UPDATE catalog_detail SET stock_available = ? WHERE id = ?")
        .bind(event.data.available)
        .bind(&event.aggregate_id)
        .execute(&db(ctx)?)
        .await?;
    Ok(())
}

/// Start the subscription that keeps `catalog_detail` up to date.
pub(crate) async fn start(executor: &RwSqlite, write_pool: SqlitePool) -> Result<Subscription> {
    SubscriptionBuilder::<RwSqlite>::new("catalog-detail")
        .handler(on_product_imported())
        .handler(on_product_details_revised())
        .handler(on_product_repriced())
        .handler(on_product_published())
        .handler(on_product_unpublished())
        .handler(on_product_archived())
        .handler(on_stock_adjusted())
        .data(write_pool)
        .all()
        .start(executor)
        .await
}
