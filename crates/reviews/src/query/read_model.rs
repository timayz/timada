//! The subscription that keeps `reviews_reviews` and `reviews_product_ratings`
//! current for every tenant.
//!
//! The handler reloads the strict write model of the review and rewrites its
//! row, then counts the product's published reviews again, in one
//! transaction, so delivery may repeat (the subscription is at-least-once)
//! without the numbers drifting. One subscription follows every tenant's
//! streams: each row carries the tenant of the event that made it, and the
//! rows live in the store's own database ([`Db`]).

use evento::{
    Executor,
    metadata::RawEvent,
    subscription::{Context, SubscriptionBuilder},
};
use timada_core::db::{Db, execute};
use timada_orders::order_projection;

use crate::aggregator::Review;
use crate::command::{ReviewState, review_projection};

/// The cursor identity of the read-model subscription, in the store.
pub const READ_MODEL_SUBSCRIPTION: &str = "timada-reviews/read-models";

/// Builds the subscription over every tenant and routing key. Start it with
/// `.start(&store)` and keep the handle for shutdown.
pub fn read_model_subscription<E: Executor>(db: Db) -> SubscriptionBuilder<E> {
    SubscriptionBuilder::new(READ_MODEL_SUBSCRIPTION)
        .data(db)
        .handler(on_review_event())
        .any_routing_key()
}

#[evento::subscription_all]
async fn on_review_event<E: Executor>(
    context: &Context<'_, E>,
    event: RawEvent<Review>,
) -> anyhow::Result<()> {
    let db: Db = context.extract();
    let Some(review) = review_projection()
        .load(&event.aggregate_id)
        .tenant(&event.tenant)
        .execute(context.executor)
        .await?
    else {
        return Ok(());
    };
    // The order is what the review was written from: it never goes away.
    let order = order_projection()
        .load(&review.order_id)
        .tenant(&event.tenant)
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
    upsert_review(&db, &event.tenant, &review, &reference, &product_name).await
}

/// Writes the row of a review as the write model sees it, and the rating of
/// its product, in one transaction.
pub async fn upsert_review(
    db: &Db,
    tenant_id: &str,
    review: &ReviewState,
    order_reference: &str,
    product_name: &str,
) -> anyhow::Result<()> {
    let env = (tenant_id, review, order_reference, product_name);
    db.write(
        &env,
        |tx, (tenant_id, review, order_reference, product_name)| {
            Box::pin(async move {
                execute(
                tx,
                "INSERT INTO reviews_reviews (tenant_id, id, order_id, order_reference, product_id,
                    product_name, variant_label, rating, title, body, author_name, photo_count,
                    status, submitted_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)
                 ON CONFLICT (tenant_id, id) DO UPDATE SET
                    rating = excluded.rating, title = excluded.title, body = excluded.body,
                    photo_count = excluded.photo_count, status = excluded.status,
                    updated_at = excluded.updated_at",
                (
                    *tenant_id,
                    review.id.as_str(),
                    review.order_id.as_str(),
                    *order_reference,
                    review.product_id.as_str(),
                    *product_name,
                    review.variant_label.as_deref(),
                    i64::from(review.rating),
                    review.title.as_deref(),
                    review.body.as_str(),
                    review.author_name.as_str(),
                    review.photos.len() as i64,
                    review.status.as_str(),
                    review.submitted_at as i64,
                    review.updated_at as i64,
                ),
            )
            .await?;
                execute(
                    tx,
                    "INSERT INTO reviews_product_ratings (tenant_id, product_id, review_count,
                    rating_sum)
                 SELECT ?1, ?2, COUNT(*), COALESCE(SUM(rating), 0) FROM reviews_reviews
                 WHERE tenant_id = ?1 AND product_id = ?2 AND status = 'published'
                 ON CONFLICT (tenant_id, product_id) DO UPDATE SET
                    review_count = excluded.review_count, rating_sum = excluded.rating_sum",
                    (*tenant_id, review.product_id.as_str()),
                )
                .await?;
                Ok(())
            })
        },
    )
    .await
}
