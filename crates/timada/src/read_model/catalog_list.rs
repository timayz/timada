//! Read model for catalog index pages: the admin catalog and the storefront
//! product grid. One denormalized row per product.

use anyhow::Result;
use evento::cursor::ReadResult;
use evento::metadata::Event;
use evento::sql::{Reader, RwSqlite};
use evento::subscription::{Context, Subscription, SubscriptionBuilder};
use evento::Executor;
use sea_query::{Expr, ExprTrait, Query};
use sqlx::SqlitePool;
use sqlx_migrator::{sqlite_migration, vec_box};

use crate::inventory::StockAdjusted;
use crate::product::{
    ProductArchived, ProductDetailsRevised, ProductImported, ProductPublished, ProductRepriced,
    ProductUnpublished,
};

pub struct M0002CreateCatalogList;

sqlite_migration!(
    M0002CreateCatalogList,
    "timada",
    "m0002_create_catalog_list",
    vec_box![],
    vec_box![(
        "CREATE TABLE catalog_list (
            id TEXT PRIMARY KEY,
            title TEXT NOT NULL,
            thumbnail_url TEXT NOT NULL DEFAULT '',
            amount_minor INTEGER NOT NULL,
            currency TEXT NOT NULL,
            status TEXT NOT NULL DEFAULT 'draft',
            provider_kind TEXT NOT NULL,
            stock_available INTEGER NOT NULL DEFAULT 0
        )",
        "DROP TABLE catalog_list"
    )]
);

#[derive(Clone, Copy, sea_query::Iden)]
pub enum CatalogList {
    Table,
    Id,
    Title,
    ThumbnailUrl,
    AmountMinor,
    Currency,
    Status,
    ProviderKind,
    StockAvailable,
}

#[derive(Debug, Clone, sqlx::FromRow, evento::Cursor)]
pub struct CatalogListRow {
    #[cursor(CatalogList::Id, 1)]
    pub id: String,
    pub title: String,
    pub thumbnail_url: String,
    pub amount_minor: i64,
    pub currency: String,
    pub status: String,
    pub provider_kind: String,
    pub stock_available: i64,
}

/// One page of the catalog, newest first (ids are ULIDs, so they sort by
/// time). `search` filters on title; `only_published` serves storefronts.
pub async fn page(
    db: &SqlitePool,
    first: u16,
    after: Option<String>,
    search: Option<&str>,
    only_published: bool,
) -> Result<ReadResult<CatalogListRow>> {
    let mut stmt = Query::select()
        .columns([
            CatalogList::Id,
            CatalogList::Title,
            CatalogList::ThumbnailUrl,
            CatalogList::AmountMinor,
            CatalogList::Currency,
            CatalogList::Status,
            CatalogList::ProviderKind,
            CatalogList::StockAvailable,
        ])
        .from(CatalogList::Table)
        .to_owned();

    if let Some(search) = search
        && !search.trim().is_empty()
    {
        stmt.and_where(Expr::col(CatalogList::Title).like(format!("%{}%", search.trim())));
    }
    if only_published {
        stmt.and_where(Expr::col(CatalogList::Status).eq("published"));
    }

    let mut reader = Reader::new(stmt);
    reader.desc().forward(first, after.map(Into::into));
    let result = reader.execute::<sqlx::Sqlite, CatalogListRow, _>(db).await?;
    Ok(result)
}

/// Total number of products, for the admin dashboard.
pub async fn count(db: &SqlitePool) -> Result<i64> {
    let (count,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM catalog_list")
        .fetch_one(db)
        .await?;
    Ok(count)
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
        "INSERT INTO catalog_list (id, title, thumbnail_url, amount_minor, currency, provider_kind)
         VALUES (?, ?, ?, ?, ?, ?) ON CONFLICT(id) DO NOTHING",
    )
    .bind(&event.aggregate_id)
    .bind(&event.data.title)
    .bind(event.data.image_urls.first().map(String::as_str).unwrap_or_default())
    .bind(event.data.price_amount_minor)
    .bind(&event.data.currency)
    .bind(&event.data.provider_kind)
    .execute(&db(ctx)?)
    .await?;
    Ok(())
}

#[evento::subscription]
async fn on_product_details_revised<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<ProductDetailsRevised>,
) -> Result<()> {
    sqlx::query("UPDATE catalog_list SET title = ? WHERE id = ?")
        .bind(&event.data.title)
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
    sqlx::query("UPDATE catalog_list SET amount_minor = ?, currency = ? WHERE id = ?")
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
    sqlx::query("UPDATE catalog_list SET status = 'published' WHERE id = ?")
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
    sqlx::query("UPDATE catalog_list SET status = 'draft' WHERE id = ?")
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
    sqlx::query("UPDATE catalog_list SET status = 'archived' WHERE id = ?")
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
    sqlx::query("UPDATE catalog_list SET stock_available = ? WHERE id = ?")
        .bind(event.data.available)
        .bind(&event.aggregate_id)
        .execute(&db(ctx)?)
        .await?;
    Ok(())
}

/// Start the subscription that keeps `catalog_list` up to date.
pub(crate) async fn start(executor: &RwSqlite, write_pool: SqlitePool) -> Result<Subscription> {
    SubscriptionBuilder::<RwSqlite>::new("catalog-list")
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
