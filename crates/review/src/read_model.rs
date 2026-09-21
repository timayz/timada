//! SQL read model: one row per review, so the product page's "16 avis clients"
//! count and average come from a single aggregate query instead of replaying
//! every review stream. Fed by the `review-product-summary` subscription.

use evento::{
    Executor,
    metadata::Event,
    subscription::{Context, SubscriptionBuilder},
};
use sqlx::{QueryBuilder, Sqlite, SqlitePool};

use crate::aggregator::{ReviewPublished, ReviewRejected, ReviewSubmitted};

/// Subscription key; the caller attaches the pool with `.data(pool)`.
pub const PRODUCT_SUMMARY_SUBSCRIPTION: &str = "review-product-summary";

#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct ProductRating {
    pub review_count: i64,
    pub average_rating: Option<f64>,
}

pub fn product_summary_subscription<E: Executor>() -> SubscriptionBuilder<E> {
    SubscriptionBuilder::new(PRODUCT_SUMMARY_SUBSCRIPTION)
        .handler(insert_on_review_submitted())
        .handler(flag_on_review_published())
        .skip::<ReviewRejected>()
        .strict()
}

/// Count and mean rating over published reviews of a product.
pub async fn product_rating(db: &SqlitePool, product_id: &str) -> sqlx::Result<ProductRating> {
    product_rating_of(db, &[product_id.to_owned()]).await
}

/// Count and mean rating over the published reviews of several products taken
/// as one — the versions of one article.
pub async fn product_rating_of(
    db: &SqlitePool,
    product_ids: &[String],
) -> sqlx::Result<ProductRating> {
    if product_ids.is_empty() {
        return Ok(ProductRating {
            review_count: 0,
            average_rating: None,
        });
    }
    let mut sql = QueryBuilder::<Sqlite>::new(
        "SELECT COUNT(*) AS review_count, AVG(rating) AS average_rating
         FROM review_product_review
         WHERE published = 1 AND product_id IN (",
    );
    let mut ids = sql.separated(", ");
    for id in product_ids {
        ids.push_bind(id);
    }
    sql.push(")");
    sql.build_query_as().fetch_one(db).await
}

fn pool<E: Executor>(ctx: &Context<'_, E>) -> anyhow::Result<SqlitePool> {
    ctx.get::<SqlitePool>()
        .ok_or_else(|| anyhow::anyhow!("SqlitePool missing from subscription context"))
}

#[evento::subscription]
async fn insert_on_review_submitted<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<ReviewSubmitted>,
) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT OR IGNORE INTO review_product_review (review_id, product_id, rating)
         VALUES (?, ?, ?)",
    )
    .bind(&event.aggregate_id)
    .bind(&event.data.product_id)
    .bind(i64::from(event.data.rating))
    .execute(&pool(ctx)?)
    .await?;
    Ok(())
}

#[evento::subscription]
async fn flag_on_review_published<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<ReviewPublished>,
) -> anyhow::Result<()> {
    sqlx::query("UPDATE review_product_review SET published = 1 WHERE review_id = ?")
        .bind(&event.aggregate_id)
        .execute(&pool(ctx)?)
        .await?;
    Ok(())
}
