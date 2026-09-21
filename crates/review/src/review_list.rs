//! SQL list read model of reviews with their text: the "Avis clients" of a
//! product page (published ones) and the admin's moderation queue. Fed by the
//! `review-list` subscription; the rating summary stays in [`crate::product_rating`].

use evento::{
    Executor,
    metadata::Event,
    subscription::{Context, SubscriptionBuilder},
};
use sqlx::{QueryBuilder, Sqlite, SqlitePool};

use crate::{
    aggregator::{ReviewPublished, ReviewRejected, ReviewSubmitted},
    value_object::ReviewStatus,
};

/// Subscription key; the caller attaches the pool with `.data(pool)`.
pub const REVIEW_LIST_SUBSCRIPTION: &str = "review-list";

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct ReviewListRow {
    pub review_id: String,
    pub product_id: String,
    pub customer_id: String,
    /// The review names the order the product was bought in.
    pub verified_purchase: bool,
    pub rating: i64,
    pub title: String,
    pub body: String,
    /// [`ReviewStatus::as_str`].
    pub status: String,
    pub rejection_reason: Option<String>,
    pub submitted_at: i64,
}

/// Filters for [`list_reviews`], the moderation queue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListReviews {
    pub status: Option<ReviewStatus>,
    pub limit: u32,
    pub offset: u32,
}

impl Default for ListReviews {
    fn default() -> Self {
        Self {
            status: None,
            limit: 50,
            offset: 0,
        }
    }
}

pub fn review_list_subscription<E: Executor>() -> SubscriptionBuilder<E> {
    SubscriptionBuilder::new(REVIEW_LIST_SUBSCRIPTION)
        .handler(insert_on_review_submitted())
        .handler(status_on_review_published())
        .handler(status_on_review_rejected())
        .strict()
}

/// The published reviews of a product, newest first.
pub async fn published_reviews(
    db: &SqlitePool,
    product_id: &str,
    limit: u32,
    offset: u32,
) -> sqlx::Result<Vec<ReviewListRow>> {
    published_reviews_of(db, &[product_id.to_owned()], limit, offset).await
}

/// The published reviews of several products taken as one — the versions of
/// one article — newest first. Each row says which product it is about.
pub async fn published_reviews_of(
    db: &SqlitePool,
    product_ids: &[String],
    limit: u32,
    offset: u32,
) -> sqlx::Result<Vec<ReviewListRow>> {
    if product_ids.is_empty() {
        return Ok(Vec::new());
    }
    let mut sql = QueryBuilder::<Sqlite>::new(
        "SELECT review_id, product_id, customer_id, verified_purchase, rating, title, body,
                status, rejection_reason, submitted_at
         FROM review_list
         WHERE status = 'published' AND product_id IN (",
    );
    let mut ids = sql.separated(", ");
    for id in product_ids {
        ids.push_bind(id);
    }
    sql.push(") ORDER BY submitted_at DESC, review_id DESC LIMIT ")
        .push_bind(limit)
        .push(" OFFSET ")
        .push_bind(offset);
    sql.build_query_as().fetch_all(db).await
}

/// Reviews across all products, oldest first: the queue is worked from the top.
pub async fn list_reviews(
    db: &SqlitePool,
    filter: &ListReviews,
) -> sqlx::Result<Vec<ReviewListRow>> {
    sqlx::query_as(
        "SELECT review_id, product_id, customer_id, verified_purchase, rating, title, body,
                status, rejection_reason, submitted_at
         FROM review_list
         WHERE (?1 IS NULL OR status = ?1)
         ORDER BY submitted_at, review_id
         LIMIT ?2 OFFSET ?3",
    )
    .bind(filter.status.as_ref().map(ReviewStatus::as_str))
    .bind(filter.limit)
    .bind(filter.offset)
    .fetch_all(db)
    .await
}

pub async fn count_reviews(db: &SqlitePool, status: Option<&ReviewStatus>) -> sqlx::Result<i64> {
    sqlx::query_scalar("SELECT COUNT(*) FROM review_list WHERE (?1 IS NULL OR status = ?1)")
        .bind(status.map(ReviewStatus::as_str))
        .fetch_one(db)
        .await
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
        "INSERT OR IGNORE INTO review_list
            (review_id, product_id, customer_id, verified_purchase, rating, title, body,
             status, submitted_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&event.aggregate_id)
    .bind(&event.data.product_id)
    .bind(&event.data.customer_id)
    .bind(event.data.order_id.is_some())
    .bind(i64::from(event.data.rating))
    .bind(&event.data.title)
    .bind(&event.data.body)
    .bind(ReviewStatus::Pending.as_str())
    .bind(event.timestamp as i64)
    .execute(&pool(ctx)?)
    .await?;
    Ok(())
}

#[evento::subscription]
async fn status_on_review_published<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<ReviewPublished>,
) -> anyhow::Result<()> {
    sqlx::query("UPDATE review_list SET status = ? WHERE review_id = ?")
        .bind(ReviewStatus::Published.as_str())
        .bind(&event.aggregate_id)
        .execute(&pool(ctx)?)
        .await?;
    Ok(())
}

#[evento::subscription]
async fn status_on_review_rejected<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<ReviewRejected>,
) -> anyhow::Result<()> {
    sqlx::query("UPDATE review_list SET status = ?, rejection_reason = ? WHERE review_id = ?")
        .bind(ReviewStatus::Rejected.as_str())
        .bind(&event.data.reason)
        .bind(&event.aggregate_id)
        .execute(&pool(ctx)?)
        .await?;
    Ok(())
}
