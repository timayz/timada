//! The subscription that keeps `catalog_products` current for one tenant.
//!
//! The handler reloads the strict write model of the stream the event belongs
//! to and rewrites its row, so delivery may repeat (the subscription is
//! at-least-once) without the row drifting. The subscription runs on the
//! tenant's own executor (its cursor lives there) and writes into the shared
//! read-model database it is given.

use evento::{
    Executor,
    metadata::RawEvent,
    subscription::{Context, SubscriptionBuilder},
};
use sqlx::SqlitePool;

use crate::aggregator::Product;
use crate::command::{ProductState, product_projection};

/// The cursor identity of the read-model subscription, in each tenant's store.
pub const READ_MODEL_SUBSCRIPTION: &str = "timada-catalog/read-models";

/// The tenant whose stream a subscription follows, handed to its handlers.
#[derive(Debug, Clone)]
pub struct TenantId(pub String);

/// Builds the subscription of one tenant over every routing key. `pool` is
/// the shared read-model database and must be allowed to write; start it
/// with `.start(&tenant_executor)` and keep the handle for shutdown.
pub fn read_model_subscription<E: Executor>(
    pool: SqlitePool,
    tenant_id: &str,
) -> SubscriptionBuilder<E> {
    SubscriptionBuilder::new(READ_MODEL_SUBSCRIPTION)
        .data(pool)
        .data(TenantId(tenant_id.to_owned()))
        .handler(on_product_event())
        .any_routing_key()
}

#[evento::subscription_all]
async fn on_product_event<E: Executor>(
    context: &Context<'_, E>,
    event: RawEvent<Product>,
) -> anyhow::Result<()> {
    let pool: SqlitePool = context.extract();
    let tenant: TenantId = context.extract();
    let Some(product) = product_projection()
        .load(&event.aggregate_id)
        .execute(context.executor)
        .await?
    else {
        return Ok(());
    };
    upsert_product(&pool, &tenant.0, &product).await
}

/// Writes the row of a product as the write model sees it.
pub async fn upsert_product(
    pool: &SqlitePool,
    tenant_id: &str,
    product: &ProductState,
) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO catalog_products (tenant_id, id, slug, name, description, amount_cents,
            currency, status, created_by, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
         ON CONFLICT (tenant_id, id) DO UPDATE SET
            slug = excluded.slug, name = excluded.name, description = excluded.description,
            amount_cents = excluded.amount_cents, currency = excluded.currency,
            status = excluded.status, updated_at = excluded.updated_at",
    )
    .bind(tenant_id)
    .bind(&product.id)
    .bind(&product.slug)
    .bind(&product.name)
    .bind(&product.description)
    .bind(product.price.as_ref().map(|p| p.amount_cents as i64))
    .bind(product.price.as_ref().map(|p| p.currency.as_str()))
    .bind(product.status.as_str())
    .bind(&product.created_by)
    .bind(product.created_at as i64)
    .bind(product.updated_at as i64)
    .execute(pool)
    .await?;
    Ok(())
}
