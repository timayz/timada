//! The subscription that keeps `catalog_products` current for every tenant.
//!
//! The handler reloads the strict write model of the stream the event belongs
//! to and rewrites its row, so delivery may repeat (the subscription is
//! at-least-once) without the row drifting. When what a category counts has
//! changed, the counts of the tenant's categories are worked out again in
//! the same transaction, so a page reads them instead of computing them. One subscription follows
//! every tenant's streams: each row carries the tenant of the event that made
//! it, and it writes into the read-model database it is given.

use evento::{
    Executor,
    metadata::RawEvent,
    subscription::{Context, SubscriptionBuilder},
};
use sqlx::SqlitePool;

use crate::aggregator::Product;
use crate::aggregator::category::Category;
use crate::aggregator::stock::StockItem;
use crate::command::{
    CategoryState, ProductState, StockState, StockSummary, category_projection, product_projection,
    stock_projection,
};
use crate::query::refresh_category_counts;

/// The cursor identity of the read-model subscription, in the store.
pub const READ_MODEL_SUBSCRIPTION: &str = "timada-catalog/read-models";

/// Builds the subscription over every tenant and routing key. `pool` is the
/// read-model database and must be allowed to write; start it with
/// `.start(&store)` and keep the handle for shutdown.
pub fn read_model_subscription<E: Executor>(pool: SqlitePool) -> SubscriptionBuilder<E> {
    SubscriptionBuilder::new(READ_MODEL_SUBSCRIPTION)
        .data(pool)
        .handler(on_product_event())
        .handler(on_category_event())
        .handler(on_stock_event())
        .any_routing_key()
}

#[evento::subscription_all]
async fn on_stock_event<E: Executor>(
    context: &Context<'_, E>,
    event: RawEvent<StockItem>,
) -> anyhow::Result<()> {
    let pool: SqlitePool = context.extract();
    let Some(stock) = stock_projection()
        .load(&event.aggregate_id)
        .tenant(&event.tenant)
        .execute(context.executor)
        .await?
    else {
        return Ok(());
    };
    let product = product_projection()
        .load(&stock.product_id)
        .tenant(&event.tenant)
        .execute(context.executor)
        .await?;
    upsert_stock(&pool, &event.tenant, &stock, product.as_ref()).await
}

/// Writes the row of one thing's stock, and the summary of its product when
/// the product is known.
pub async fn upsert_stock(
    pool: &SqlitePool,
    tenant_id: &str,
    stock: &StockState,
    product: Option<&ProductState>,
) -> anyhow::Result<()> {
    let mut tx = pool.begin().await?;
    sqlx::query(
        "INSERT INTO catalog_stock (tenant_id, product_id, variant_id, tracked, on_hand, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT (tenant_id, product_id, variant_id) DO UPDATE SET
            tracked = excluded.tracked, on_hand = excluded.on_hand,
            updated_at = excluded.updated_at",
    )
    .bind(tenant_id)
    .bind(&stock.product_id)
    .bind(stock.variant_id.as_deref().unwrap_or(""))
    .bind(stock.tracked)
    .bind(i64::from(stock.on_hand))
    .bind(stock.updated_at as i64)
    .execute(&mut *tx)
    .await?;
    if let Some(product) = product {
        refresh_stock_state(&mut tx, tenant_id, product).await?;
    }
    tx.commit().await?;
    Ok(())
}

/// Works out a product's stock summary again from its stock rows.
async fn refresh_stock_state(
    tx: &mut sqlx::SqliteConnection,
    tenant_id: &str,
    product: &ProductState,
) -> anyhow::Result<()> {
    let rows: Vec<(String, bool, i64)> = sqlx::query_as(
        "SELECT variant_id, tracked, on_hand FROM catalog_stock
         WHERE tenant_id = ?1 AND product_id = ?2",
    )
    .bind(tenant_id)
    .bind(&product.id)
    .fetch_all(&mut *tx)
    .await?;
    let summary = StockSummary::of(product, |variant_id| {
        rows.iter()
            .find(|(id, tracked, _)| *tracked && id == variant_id.unwrap_or(""))
            .map(|(_, _, on_hand)| (*on_hand).max(0) as u32)
    });
    sqlx::query("UPDATE catalog_products SET stock_state = ?3 WHERE tenant_id = ?1 AND id = ?2")
        .bind(tenant_id)
        .bind(&product.id)
        .bind(summary.as_str())
        .execute(&mut *tx)
        .await?;
    Ok(())
}

#[evento::subscription_all]
async fn on_product_event<E: Executor>(
    context: &Context<'_, E>,
    event: RawEvent<Product>,
) -> anyhow::Result<()> {
    let pool: SqlitePool = context.extract();
    let Some(product) = product_projection()
        .load(&event.aggregate_id)
        .tenant(&event.tenant)
        .execute(context.executor)
        .await?
    else {
        return Ok(());
    };
    upsert_product(&pool, &event.tenant, &product).await
}

#[evento::subscription_all]
async fn on_category_event<E: Executor>(
    context: &Context<'_, E>,
    event: RawEvent<Category>,
) -> anyhow::Result<()> {
    let pool: SqlitePool = context.extract();
    let Some(category) = category_projection()
        .load(&event.aggregate_id)
        .tenant(&event.tenant)
        .execute(context.executor)
        .await?
    else {
        return Ok(());
    };
    upsert_category(&pool, &event.tenant, &category).await
}

/// Writes the row of a product and its category memberships as the write
/// model sees them, in one transaction: the row, its memberships and the
/// category counts they change are never half-written.
pub async fn upsert_product(
    pool: &SqlitePool,
    tenant_id: &str,
    product: &ProductState,
) -> anyhow::Result<()> {
    let mut tx = pool.begin().await?;
    let was: Option<(bool, String, String)> = sqlx::query_as(
        "SELECT status <> 'archived', slug, name FROM catalog_products
         WHERE tenant_id = ?1 AND id = ?2",
    )
    .bind(tenant_id)
    .bind(&product.id)
    .fetch_optional(&mut *tx)
    .await?;
    let was_counted = was.as_ref().map(|(counted, _, _)| *counted);
    let was_in: Vec<String> = sqlx::query_scalar(
        "SELECT category_id FROM catalog_product_categories
         WHERE tenant_id = ?1 AND product_id = ?2 ORDER BY category_id",
    )
    .bind(tenant_id)
    .bind(&product.id)
    .fetch_all(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO catalog_products (tenant_id, id, slug, name, description, amount_cents,
            currency, status, created_by, created_at, updated_at, thumbnail, image_count,
            min_amount_cents, max_amount_cents, variant_count)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)
         ON CONFLICT (tenant_id, id) DO UPDATE SET
            slug = excluded.slug, name = excluded.name, description = excluded.description,
            amount_cents = excluded.amount_cents, currency = excluded.currency,
            status = excluded.status, updated_at = excluded.updated_at,
            thumbnail = excluded.thumbnail, image_count = excluded.image_count,
            min_amount_cents = excluded.min_amount_cents,
            max_amount_cents = excluded.max_amount_cents,
            variant_count = excluded.variant_count",
    )
    .bind(tenant_id)
    .bind(&product.id)
    .bind(&product.slug)
    .bind(&product.name)
    .bind(&product.description)
    .bind(product.price.as_ref().map(|p| p.amount_cents as i64))
    // The currency everything on this product is priced in, which for one
    // sold only through its variants is theirs rather than a base price's.
    .bind(product.currency())
    .bind(product.status.as_str())
    .bind(&product.created_by)
    .bind(product.created_at as i64)
    .bind(product.updated_at as i64)
    .bind(product.images.first().map(|image| image.file()))
    .bind(product.images.len() as i64)
    .bind(product.price_range().map(|(min, _, _)| min as i64))
    .bind(product.price_range().map(|(_, max, _)| max as i64))
    .bind(product.variants.len() as i64)
    .execute(&mut *tx)
    .await?;
    sqlx::query("DELETE FROM catalog_product_categories WHERE tenant_id = ?1 AND product_id = ?2")
        .bind(tenant_id)
        .bind(&product.id)
        .execute(&mut *tx)
        .await?;
    for category_id in &product.categories {
        sqlx::query(
            "INSERT INTO catalog_product_categories (tenant_id, product_id, category_id)
             VALUES (?1, ?2, ?3)",
        )
        .bind(tenant_id)
        .bind(&product.id)
        .bind(category_id)
        .execute(&mut *tx)
        .await?;
    }
    // What a search finds it by, rewritten only when that changed.
    if was
        .as_ref()
        .is_none_or(|(_, slug, name)| *slug != product.slug || *name != product.name)
    {
        sqlx::query(
            "DELETE FROM catalog_products_fts WHERE rowid =
                (SELECT rowid FROM catalog_products WHERE tenant_id = ?1 AND id = ?2)",
        )
        .bind(tenant_id)
        .bind(&product.id)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "INSERT INTO catalog_products_fts (rowid, tenant_id, slug, name)
             SELECT rowid, tenant_id, slug, name FROM catalog_products
             WHERE tenant_id = ?1 AND id = ?2",
        )
        .bind(tenant_id)
        .bind(&product.id)
        .execute(&mut *tx)
        .await?;
    }
    let is_counted = product.status.as_str() != "archived";
    // A rename or a new price leaves every count as it was.
    if was_counted.unwrap_or(false) != is_counted
        || (is_counted && !was_in.iter().eq(product.categories.iter()))
    {
        refresh_category_counts(&mut tx, tenant_id).await?;
    }
    // A variant added or removed changes what the summary counts.
    refresh_stock_state(&mut tx, tenant_id, product).await?;
    tx.commit().await?;
    Ok(())
}

/// Writes the row of a category as the write model sees it, and works the
/// counts out again when it is new, moved or archived.
pub async fn upsert_category(
    pool: &SqlitePool,
    tenant_id: &str,
    category: &CategoryState,
) -> anyhow::Result<()> {
    let mut tx = pool.begin().await?;
    let was: Option<(Option<String>, bool)> = sqlx::query_as(
        "SELECT parent_id, archived FROM catalog_categories WHERE tenant_id = ?1 AND id = ?2",
    )
    .bind(tenant_id)
    .bind(&category.id)
    .fetch_optional(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO catalog_categories (tenant_id, id, slug, name, description, parent_id,
            position, archived, in_menu, created_by, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
         ON CONFLICT (tenant_id, id) DO UPDATE SET
            slug = excluded.slug, name = excluded.name, description = excluded.description,
            parent_id = excluded.parent_id, position = excluded.position,
            archived = excluded.archived, in_menu = excluded.in_menu,
            updated_at = excluded.updated_at",
    )
    .bind(tenant_id)
    .bind(&category.id)
    .bind(&category.slug)
    .bind(&category.name)
    .bind(&category.description)
    .bind(category.parent_id.as_deref())
    .bind(category.position as i64)
    .bind(category.archived)
    .bind(!category.hidden_from_menu)
    .bind(&category.created_by)
    .bind(category.created_at as i64)
    .bind(category.updated_at as i64)
    .execute(&mut *tx)
    .await?;
    if was != Some((category.parent_id.clone(), category.archived)) {
        refresh_category_counts(&mut tx, tenant_id).await?;
    }
    tx.commit().await?;
    Ok(())
}
