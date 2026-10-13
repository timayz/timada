//! The subscription that keeps the local product rows current from the
//! store's catalog events. The handler reloads the product's write model from
//! the store (its snapshot is local too) and rewrites the row, so a repeated
//! delivery changes nothing.

use evento::{
    Executor,
    metadata::RawEvent,
    subscription::{Context, SubscriptionBuilder},
};
use sqlx::SqlitePool;
use timada_catalog::aggregator::Product;
use timada_catalog::aggregator::category::Category;
use timada_catalog::{CategoryState, ProductState, category_projection, product_projection};

/// The cursor identity of the catalog subscription, in the local database.
pub const CATALOG_SUBSCRIPTION: &str = "storefront/catalog";

/// Builds the subscription over every routing key of the store. `pool` is the
/// local database; start it with `.start(&executor)`.
pub fn catalog_subscription<E: Executor>(pool: SqlitePool) -> SubscriptionBuilder<E> {
    SubscriptionBuilder::new(CATALOG_SUBSCRIPTION)
        .data(pool)
        .handler(on_product_event())
        .handler(on_category_event())
        .any_routing_key()
}

#[evento::subscription_all]
async fn on_product_event<E: Executor>(
    context: &Context<'_, E>,
    event: RawEvent<Product>,
) -> anyhow::Result<()> {
    let pool: SqlitePool = context.extract();
    let Some(product) = product_projection()
        .load(&event.aggregate_id)
        .execute(context.executor)
        .await?
    else {
        return Ok(());
    };
    upsert_product(&pool, &product).await
}

#[evento::subscription_all]
async fn on_category_event<E: Executor>(
    context: &Context<'_, E>,
    event: RawEvent<Category>,
) -> anyhow::Result<()> {
    let pool: SqlitePool = context.extract();
    let Some(category) = category_projection()
        .load(&event.aggregate_id)
        .execute(context.executor)
        .await?
    else {
        return Ok(());
    };
    upsert_category(&pool, &category).await
}

/// The product row and the categories it is in, in one transaction: a page
/// never sees a product filed half-way.
async fn upsert_product(pool: &SqlitePool, product: &ProductState) -> anyhow::Result<()> {
    let mut tx = pool.begin().await?;
    sqlx::query(
        "INSERT INTO storefront_products (id, slug, name, description, amount_cents, currency,
            status, updated_at, images_json)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
         ON CONFLICT (id) DO UPDATE SET
            slug = excluded.slug, name = excluded.name, description = excluded.description,
            amount_cents = excluded.amount_cents, currency = excluded.currency,
            status = excluded.status, updated_at = excluded.updated_at,
            images_json = excluded.images_json",
    )
    .bind(&product.id)
    .bind(&product.slug)
    .bind(&product.name)
    .bind(&product.description)
    .bind(product.price.as_ref().map(|p| p.amount_cents as i64))
    .bind(product.price.as_ref().map(|p| p.currency.as_str()))
    .bind(product.status.as_str())
    .bind(product.updated_at as i64)
    .bind(images_json(product)?)
    .execute(&mut *tx)
    .await?;
    sqlx::query("DELETE FROM storefront_product_categories WHERE product_id = ?1")
        .bind(&product.id)
        .execute(&mut *tx)
        .await?;
    for category_id in &product.categories {
        sqlx::query(
            "INSERT INTO storefront_product_categories (product_id, category_id)
             VALUES (?1, ?2)",
        )
        .bind(&product.id)
        .bind(category_id)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(())
}

/// The gallery as the local rows carry it.
fn images_json(product: &ProductState) -> anyhow::Result<String> {
    let images: Vec<crate::Image> = product
        .images
        .iter()
        .map(|image| crate::Image {
            id: image.id.clone(),
            file: image.file(),
            mime: image.mime.clone(),
            alt: image.alt.clone(),
            width: image.width,
            height: image.height,
            // Filled in when a page reads the product: the address belongs
            // to the host's configuration, not to the row.
            url: String::new(),
        })
        .collect();
    Ok(serde_json::to_string(&images)?)
}

async fn upsert_category(pool: &SqlitePool, category: &CategoryState) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO storefront_categories (id, slug, name, description, parent_id, position,
            archived, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT (id) DO UPDATE SET
            slug = excluded.slug, name = excluded.name, description = excluded.description,
            parent_id = excluded.parent_id, position = excluded.position,
            archived = excluded.archived, updated_at = excluded.updated_at",
    )
    .bind(&category.id)
    .bind(&category.slug)
    .bind(&category.name)
    .bind(&category.description)
    .bind(category.parent_id.as_deref())
    .bind(category.position as i64)
    .bind(category.archived)
    .bind(category.updated_at as i64)
    .execute(pool)
    .await?;
    Ok(())
}
