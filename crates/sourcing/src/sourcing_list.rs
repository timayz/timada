//! SQL list read models behind the back office: the suppliers, and what each
//! one sources. Fed by the `sourcing-catalogue` subscription.
//!
//! Every handler rewrites a whole row from the aggregate's view rather than
//! nudging columns, so a redelivered event changes nothing.

use evento::{
    Executor,
    metadata::Event,
    subscription::{Context, SubscriptionBuilder},
};
use sqlx::SqlitePool;
use timada_core::Money;

use crate::{
    aggregator::{
        ProductSourced, SourcePriceLocked, SourcePriceUnlocked, SourcingStopped,
        SupplierRegistered, SupplierRenamed, SupplierResumed, SupplierSuspended,
    },
    query::{load_sourced_product_view, load_supplier_view},
};

/// Subscription key; the caller attaches the pool with `.data(pool)`.
pub const SOURCING_LIST_SUBSCRIPTION: &str = "sourcing-catalogue";

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct SupplierRow {
    pub supplier_id: String,
    pub slug: String,
    pub name: String,
    pub connector: String,
    pub currency: String,
    pub suspended: bool,
    pub suspended_reason: Option<String>,
    pub registered_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct SourcedRow {
    pub product_id: String,
    pub sourced_id: String,
    pub supplier_id: String,
    pub external_item_id: String,
    pub external_sku: Option<String>,
    pub locked: bool,
    pub locked_reason: Option<String>,
    pub active: bool,
    pub sourced_at: i64,
}

/// The last thing a supplier said about an item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OfferRow {
    pub product_id: String,
    pub supplier_id: String,
    pub cost: Money,
    pub shipping: Money,
    pub available: i64,
    pub title: Option<String>,
    pub url: Option<String>,
    /// The cost in the currency the product is sold in, once converted.
    pub landed: Option<Money>,
    /// The rate it was converted at, as `(millionths, source, as of)`.
    pub rate: Option<(i64, String, i64)>,
    pub fetched_at: i64,
}

pub fn sourcing_list_subscription<E: Executor>() -> SubscriptionBuilder<E> {
    SubscriptionBuilder::new(SOURCING_LIST_SUBSCRIPTION)
        .handler(refresh_on_supplier_registered())
        .handler(refresh_on_supplier_renamed())
        .handler(refresh_on_supplier_suspended())
        .handler(refresh_on_supplier_resumed())
        .handler(refresh_on_product_sourced())
        .handler(refresh_on_sourcing_stopped())
        .handler(refresh_on_source_price_locked())
        .handler(refresh_on_source_price_unlocked())
        .strict()
}

pub async fn list_suppliers(db: &SqlitePool) -> sqlx::Result<Vec<SupplierRow>> {
    sqlx::query_as(
        "SELECT supplier_id, slug, name, connector, currency, suspended,
                suspended_reason, registered_at
         FROM sourcing_supplier
         ORDER BY name COLLATE NOCASE, supplier_id",
    )
    .fetch_all(db)
    .await
}

pub async fn supplier_by_id(db: &SqlitePool, id: &str) -> sqlx::Result<Option<SupplierRow>> {
    sqlx::query_as(
        "SELECT supplier_id, slug, name, connector, currency, suspended,
                suspended_reason, registered_at
         FROM sourcing_supplier WHERE supplier_id = ?",
    )
    .bind(id)
    .fetch_optional(db)
    .await
}

/// What one supplier sources, newest link first.
pub async fn sourced_of_supplier(
    db: &SqlitePool,
    supplier_id: &str,
    limit: u32,
    offset: u32,
) -> sqlx::Result<Vec<SourcedRow>> {
    sqlx::query_as(
        "SELECT product_id, sourced_id, supplier_id, external_item_id, external_sku,
                locked, locked_reason, active, sourced_at
         FROM sourcing_product
         WHERE supplier_id = ?1 AND active = 1
         ORDER BY sourced_at DESC, product_id
         LIMIT ?2 OFFSET ?3",
    )
    .bind(supplier_id)
    .bind(limit)
    .bind(offset)
    .fetch_all(db)
    .await
}

pub async fn count_sourced_of_supplier(db: &SqlitePool, supplier_id: &str) -> sqlx::Result<i64> {
    sqlx::query_scalar("SELECT COUNT(*) FROM sourcing_product WHERE supplier_id = ? AND active = 1")
        .bind(supplier_id)
        .fetch_one(db)
        .await
}

/// Where one product is bought, if it is.
pub async fn sourced_of_product(
    db: &SqlitePool,
    product_id: &str,
) -> sqlx::Result<Option<SourcedRow>> {
    sqlx::query_as(
        "SELECT product_id, sourced_id, supplier_id, external_item_id, external_sku,
                locked, locked_reason, active, sourced_at
         FROM sourcing_product WHERE product_id = ? AND active = 1",
    )
    .bind(product_id)
    .fetch_optional(db)
    .await
}

/// Which of these products are bought from a supplier — what a page asking
/// about a basketful of lines needs, in one query.
pub async fn sourced_products_by_ids(
    db: &SqlitePool,
    product_ids: &[String],
) -> sqlx::Result<Vec<SourcedRow>> {
    if product_ids.is_empty() {
        return Ok(Vec::new());
    }
    let mut builder = sqlx::QueryBuilder::new(
        "SELECT product_id, sourced_id, supplier_id, external_item_id, external_sku,
                locked, locked_reason, active, sourced_at
         FROM sourcing_product WHERE active = 1 AND product_id IN (",
    );
    let mut separated = builder.separated(", ");
    for product_id in product_ids {
        separated.push_bind(product_id);
    }
    separated.push_unseparated(")");
    builder.build_query_as().fetch_all(db).await
}

pub async fn offer_of_product(db: &SqlitePool, product_id: &str) -> sqlx::Result<Option<OfferRow>> {
    let row: Option<RawOffer> = sqlx::query_as(
        "SELECT product_id, supplier_id, cost_minor, cost_currency, shipping_minor,
                available, title, url, landed_minor, landed_currency,
                rate_micros, rate_source, rate_as_of, fetched_at
         FROM sourcing_offer WHERE product_id = ?",
    )
    .bind(product_id)
    .fetch_optional(db)
    .await?;
    Ok(row.map(Into::into))
}

#[derive(sqlx::FromRow)]
struct RawOffer {
    product_id: String,
    supplier_id: String,
    cost_minor: i64,
    cost_currency: String,
    shipping_minor: i64,
    available: i64,
    title: Option<String>,
    url: Option<String>,
    landed_minor: Option<i64>,
    landed_currency: Option<String>,
    rate_micros: Option<i64>,
    rate_source: Option<String>,
    rate_as_of: Option<i64>,
    fetched_at: i64,
}

impl From<RawOffer> for OfferRow {
    fn from(row: RawOffer) -> Self {
        let landed = row
            .landed_minor
            .zip(row.landed_currency)
            .map(|(minor, currency)| Money::new(minor, currency));
        let rate = match (row.rate_micros, row.rate_source, row.rate_as_of) {
            (Some(micros), Some(source), Some(as_of)) => Some((micros, source, as_of)),
            _ => None,
        };
        Self {
            cost: Money::new(row.cost_minor, row.cost_currency.clone()),
            shipping: Money::new(row.shipping_minor, row.cost_currency),
            product_id: row.product_id,
            supplier_id: row.supplier_id,
            available: row.available,
            title: row.title,
            url: row.url,
            landed,
            rate,
            fetched_at: row.fetched_at,
        }
    }
}

fn pool<E: Executor>(ctx: &Context<'_, E>) -> anyhow::Result<SqlitePool> {
    ctx.get::<SqlitePool>()
        .ok_or_else(|| anyhow::anyhow!("SqlitePool missing from subscription context"))
}

async fn refresh_supplier<E: Executor>(
    ctx: &Context<'_, E>,
    supplier_id: &str,
) -> anyhow::Result<()> {
    let Some(supplier) = load_supplier_view(ctx.executor, supplier_id).await? else {
        anyhow::bail!("supplier {supplier_id} cannot be loaded");
    };
    sqlx::query(
        "INSERT INTO sourcing_supplier
            (supplier_id, slug, name, connector, currency, suspended, suspended_reason,
             registered_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT (supplier_id) DO UPDATE SET
            name = excluded.name,
            connector = excluded.connector,
            currency = excluded.currency,
            suspended = excluded.suspended,
            suspended_reason = excluded.suspended_reason",
    )
    .bind(&supplier.id)
    .bind(&supplier.slug)
    .bind(&supplier.name)
    .bind(&supplier.connector)
    .bind(&supplier.currency)
    .bind(supplier.suspended)
    .bind((!supplier.suspended_reason.is_empty()).then_some(supplier.suspended_reason.as_str()))
    .bind(supplier.registered_at as i64)
    .execute(&pool(ctx)?)
    .await?;
    Ok(())
}

async fn refresh_sourced<E: Executor>(
    ctx: &Context<'_, E>,
    sourced_id: &str,
) -> anyhow::Result<()> {
    let Some(sourced) = load_sourced_product_view(ctx.executor, sourced_id).await? else {
        anyhow::bail!("sourced product {sourced_id} cannot be loaded");
    };
    sqlx::query(
        "INSERT INTO sourcing_product
            (product_id, sourced_id, supplier_id, external_item_id, external_sku,
             locked, locked_reason, active, sourced_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT (product_id) DO UPDATE SET
            sourced_id = excluded.sourced_id,
            supplier_id = excluded.supplier_id,
            external_item_id = excluded.external_item_id,
            external_sku = excluded.external_sku,
            locked = excluded.locked,
            locked_reason = excluded.locked_reason,
            active = excluded.active,
            sourced_at = excluded.sourced_at",
    )
    .bind(&sourced.product_id)
    .bind(&sourced.id)
    .bind(&sourced.supplier_id)
    .bind(&sourced.external_item_id)
    .bind(sourced.external_sku.as_deref())
    .bind(sourced.locked)
    .bind((!sourced.locked_reason.is_empty()).then_some(sourced.locked_reason.as_str()))
    .bind(sourced.active)
    .bind(sourced.sourced_at as i64)
    .execute(&pool(ctx)?)
    .await?;
    Ok(())
}

#[evento::subscription]
async fn refresh_on_supplier_registered<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<SupplierRegistered>,
) -> anyhow::Result<()> {
    refresh_supplier(ctx, &event.aggregate_id).await
}

#[evento::subscription]
async fn refresh_on_supplier_renamed<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<SupplierRenamed>,
) -> anyhow::Result<()> {
    refresh_supplier(ctx, &event.aggregate_id).await
}

#[evento::subscription]
async fn refresh_on_supplier_suspended<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<SupplierSuspended>,
) -> anyhow::Result<()> {
    refresh_supplier(ctx, &event.aggregate_id).await
}

#[evento::subscription]
async fn refresh_on_supplier_resumed<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<SupplierResumed>,
) -> anyhow::Result<()> {
    refresh_supplier(ctx, &event.aggregate_id).await
}

#[evento::subscription]
async fn refresh_on_product_sourced<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<ProductSourced>,
) -> anyhow::Result<()> {
    refresh_sourced(ctx, &event.aggregate_id).await
}

#[evento::subscription]
async fn refresh_on_sourcing_stopped<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<SourcingStopped>,
) -> anyhow::Result<()> {
    refresh_sourced(ctx, &event.aggregate_id).await
}

#[evento::subscription]
async fn refresh_on_source_price_locked<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<SourcePriceLocked>,
) -> anyhow::Result<()> {
    refresh_sourced(ctx, &event.aggregate_id).await
}

#[evento::subscription]
async fn refresh_on_source_price_unlocked<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<SourcePriceUnlocked>,
) -> anyhow::Result<()> {
    refresh_sourced(ctx, &event.aggregate_id).await
}
