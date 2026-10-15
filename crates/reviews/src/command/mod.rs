//! Write side: the command gateway and the strict write model it loads.
//!
//! The write model folds *all* events of a review (`strict()`), so the version
//! it observes, and `write()` sends as `original_version`, never goes stale.
//! It is not snapshotted: a review is a handful of events.

mod review;

use std::ops::Deref;

pub use review::*;

use evento::{Executor, Projection, metadata::Event};

use crate::aggregator::{PhotoRemoved, Published, Rejected, Submitted};
use crate::error::ReviewError;
use crate::value_object::{ReviewPhoto, ReviewStatus};

/// The command gateway. Cheap to clone when `E` is.
pub struct Command<E: Executor>(pub E);

impl<E: Executor> Deref for Command<E> {
    type Target = E;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[evento::projection(id = id, name = "timada-reviews/ReviewState")]
#[evento::snapshot(none)]
#[derive(Debug, PartialEq, Eq)]
pub struct ReviewState {
    pub id: String,
    pub order_id: String,
    pub product_id: String,
    pub variant_label: Option<String>,
    pub rating: u8,
    pub title: Option<String>,
    pub body: String,
    pub author_name: String,
    /// The photos still on it.
    pub photos: Vec<ReviewPhoto>,
    pub status: ReviewStatus,
    /// Why it was rejected, once it is.
    pub rejection_reason: Option<String>,
    /// Unix seconds of `Submitted`.
    pub submitted_at: u64,
    /// Unix seconds of `Published`, while it is.
    pub published_at: Option<u64>,
    /// Unix seconds of the last event.
    pub updated_at: u64,
}

impl ReviewState {
    pub fn is_published(&self) -> bool {
        self.status == ReviewStatus::Published
    }

    pub fn is_rejected(&self) -> bool {
        self.status == ReviewStatus::Rejected
    }
}

pub fn review_projection<E: Executor>() -> Projection<E, ReviewState> {
    Projection::new::<crate::aggregator::Review>()
        .handler(on_submitted())
        .handler(on_published())
        .handler(on_rejected())
        .handler(on_photo_removed())
        .strict()
}

#[evento::handler]
async fn on_submitted(event: Event<Submitted>, review: &mut ReviewState) -> anyhow::Result<()> {
    let data = &event.data;
    review.id = event.aggregate_id.to_owned();
    review.order_id = data.order_id.to_owned();
    review.product_id = data.product_id.to_owned();
    review.variant_label = data.variant_label.to_owned();
    review.rating = data.rating;
    review.title = data.title.to_owned();
    review.body = data.body.to_owned();
    review.author_name = data.author_name.to_owned();
    review.photos = data.photos.to_owned();
    review.status = ReviewStatus::Pending;
    review.submitted_at = event.timestamp;
    review.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_published(event: Event<Published>, review: &mut ReviewState) -> anyhow::Result<()> {
    review.status = ReviewStatus::Published;
    review.published_at = Some(event.timestamp);
    review.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_rejected(event: Event<Rejected>, review: &mut ReviewState) -> anyhow::Result<()> {
    review.status = ReviewStatus::Rejected;
    review.rejection_reason = Some(event.data.reason.to_owned());
    review.published_at = None;
    review.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_photo_removed(
    event: Event<PhotoRemoved>,
    review: &mut ReviewState,
) -> anyhow::Result<()> {
    review
        .photos
        .retain(|photo| photo.id != event.data.image_id);
    review.updated_at = event.timestamp;
    Ok(())
}

impl<E: Executor> Command<E> {
    pub async fn load_review(&self, review_id: &str) -> anyhow::Result<Option<ReviewState>> {
        review_projection().load(review_id).execute(&self.0).await
    }

    /// The review, or `NotFound`; `Rejected` once it is: every moderation
    /// starts here.
    pub(crate) async fn require_open(&self, review_id: &str) -> Result<ReviewState, ReviewError> {
        let review = self
            .load_review(review_id)
            .await?
            .ok_or(ReviewError::NotFound)?;
        if review.is_rejected() {
            return Err(ReviewError::Rejected);
        }
        Ok(review)
    }
}
