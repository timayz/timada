//! One review as shown under "Avis clients", folded from one `Review` stream.
//! Executor-backed snapshots via the bitcode derives.

use evento::{Executor, metadata::Event, projection::Projection};

use crate::{
    aggregator::{Review, ReviewPublished, ReviewRejected, ReviewSubmitted},
    value_object::ReviewStatus,
};

#[evento::projection(bitcode::Encode, bitcode::Decode)]
#[derive(Debug, PartialEq)]
pub struct ReviewView {
    pub id: String,
    pub product_id: String,
    pub customer_id: String,
    pub order_id: Option<String>,
    pub rating: u8,
    pub title: String,
    pub body: String,
    pub status: ReviewStatus,
    pub rejection_reason: Option<String>,
}

pub fn create_projection<E: Executor>() -> Projection<E, ReviewView> {
    Projection::new::<Review>()
        .handler(on_review_submitted())
        .handler(on_review_published())
        .handler(on_review_rejected())
        .strict()
}

pub async fn load<E: Executor>(
    executor: &E,
    id: impl Into<String>,
) -> anyhow::Result<Option<ReviewView>> {
    create_projection().load(id).execute(executor).await
}

#[evento::handler]
async fn on_review_submitted(
    event: Event<ReviewSubmitted>,
    row: &mut ReviewView,
) -> anyhow::Result<()> {
    row.id = event.aggregate_id.to_owned();
    row.product_id = event.data.product_id;
    row.customer_id = event.data.customer_id;
    row.order_id = event.data.order_id;
    row.rating = event.data.rating;
    row.title = event.data.title;
    row.body = event.data.body;
    row.status = ReviewStatus::Pending;
    Ok(())
}

#[evento::handler]
async fn on_review_published(
    _event: Event<ReviewPublished>,
    row: &mut ReviewView,
) -> anyhow::Result<()> {
    row.status = ReviewStatus::Published;
    Ok(())
}

#[evento::handler]
async fn on_review_rejected(
    event: Event<ReviewRejected>,
    row: &mut ReviewView,
) -> anyhow::Result<()> {
    row.status = ReviewStatus::Rejected;
    row.rejection_reason = Some(event.data.reason);
    Ok(())
}
