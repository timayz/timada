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
use timada_catalog::{ProductState, product_projection};

/// The cursor identity of the catalog subscription, in the local database.
pub const CATALOG_SUBSCRIPTION: &str = "storefront/catalog";

/// Builds the subscription over every routing key of the store. `pool` is the
/// local database; start it with `.start(&executor)`.
pub fn catalog_subscription<E: Executor>(pool: SqlitePool) -> SubscriptionBuilder<E> {
    SubscriptionBuilder::new(CATALOG_SUBSCRIPTION)
        .data(pool)
        .handler(on_product_event())
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

async fn upsert_product(pool: &SqlitePool, product: &ProductState) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO storefront_products (id, slug, name, description, amount_cents, currency,
            status, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT (id) DO UPDATE SET
            slug = excluded.slug, name = excluded.name, description = excluded.description,
            amount_cents = excluded.amount_cents, currency = excluded.currency,
            status = excluded.status, updated_at = excluded.updated_at",
    )
    .bind(&product.id)
    .bind(&product.slug)
    .bind(&product.name)
    .bind(&product.description)
    .bind(product.price.as_ref().map(|p| p.amount_cents as i64))
    .bind(product.price.as_ref().map(|p| p.currency.as_str()))
    .bind(product.status.as_str())
    .bind(product.updated_at as i64)
    .execute(pool)
    .await?;
    Ok(())
}
