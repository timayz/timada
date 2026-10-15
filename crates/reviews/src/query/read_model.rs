//! The subscription that keeps `reviews_reviews` and `reviews_product_ratings`
//! current for one tenant.
//!
//! The handler reloads the strict write model of the review and rewrites its
//! row, then counts the product's published reviews again, in one
//! transaction, so delivery may repeat (the subscription is at-least-once)
//! without the numbers drifting. It runs on the tenant's own executor (its
//! cursor lives there) and writes into the shared read-model database it is
//! given.

use evento::{
    Executor,
    metadata::RawEvent,
    subscription::{Context, SubscriptionBuilder},
};
use sqlx::SqlitePool;
use timada_orders::order_projection;

use crate::aggregator::Review;
use crate::command::{ReviewState, review_projection};

/// The cursor identity of the read-model subscription, in each tenant's store.
pub const READ_MODEL_SUBSCRIPTION: &str = "timada-reviews/read-models";

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
        .handler(on_review_event())
        .any_routing_key()
}

#[evento::subscription_all]
async fn on_review_event<E: Executor>(
    context: &Context<'_, E>,
    event: RawEvent<Review>,
) -> anyhow::Result<()> {
    let pool: SqlitePool = context.extract();
    let tenant: TenantId = context.extract();
    let Some(review) = review_projection()
        .load(&event.aggregate_id)
        .execute(context.executor)
        .await?
    else {
        return Ok(());
    };
    // The order is what the review was written from: it never goes away.
    let order = order_projection()
        .load(&review.order_id)
        .execute(context.executor)
        .await?;
    let (reference, product_name) = order
        .map(|order| {
            let name = order
                .lines
                .iter()
                .find(|line| line.product_id == review.product_id)
                .map(|line| line.name.clone())
                .unwrap_or_default();
            (order.reference, name)
        })
        .unwrap_or_default();
    upsert_review(&pool, &tenant.0, &review, &reference, &product_name).await
}

/// Writes the row of a review as the write model sees it, and the rating of
/// its product.
pub async fn upsert_review(
    pool: &SqlitePool,
    tenant_id: &str,
    review: &ReviewState,
    order_reference: &str,
    product_name: &str,
) -> anyhow::Result<()> {
    let mut tx = pool.begin().await?;
    sqlx::query(
        "INSERT INTO reviews_reviews (tenant_id, id, order_id, order_reference, product_id,
            product_name, variant_label, rating, title, body, author_name, photo_count, status,
            submitted_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)
         ON CONFLICT (tenant_id, id) DO UPDATE SET
            photo_count = excluded.photo_count, status = excluded.status,
            updated_at = excluded.updated_at",
    )
    .bind(tenant_id)
    .bind(&review.id)
    .bind(&review.order_id)
    .bind(order_reference)
    .bind(&review.product_id)
    .bind(product_name)
    .bind(&review.variant_label)
    .bind(i64::from(review.rating))
    .bind(&review.title)
    .bind(&review.body)
    .bind(&review.author_name)
    .bind(review.photos.len() as i64)
    .bind(review.status.as_str())
    .bind(review.submitted_at as i64)
    .bind(review.updated_at as i64)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO reviews_product_ratings (tenant_id, product_id, review_count, rating_sum)
         SELECT ?1, ?2, COUNT(*), COALESCE(SUM(rating), 0) FROM reviews_reviews
         WHERE tenant_id = ?1 AND product_id = ?2 AND status = 'published'
         ON CONFLICT (tenant_id, product_id) DO UPDATE SET
            review_count = excluded.review_count, rating_sum = excluded.rating_sum",
    )
    .bind(tenant_id)
    .bind(&review.product_id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}
