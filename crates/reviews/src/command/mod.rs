//! Write side: the command gateway and the strict write model it loads.
//!
//! The write model folds *all* events of a review (`strict()`), so the version
//! it observes, and `write()` sends as `original_version`, never goes stale.
//! It is not snapshotted: a review is a handful of events.

mod request;
mod review;

use std::ops::Deref;

pub use review::*;

use evento::{Executor, Projection, metadata::Event};

use crate::aggregator::{
    LinkIssued, PhotoRemoved, Published, Rejected, Replied, ReplyRemoved, RequestDelaySet,
    RequestMailed, RequestsDisabled, Revised, Submitted,
};
use crate::error::ReviewError;
use crate::value_object::{
    DEFAULT_REQUEST_DELAY_DAYS, ReviewPhoto, ReviewStatus, review_request_id, review_requests_id,
};

/// The command gateway. Cheap to clone when `E` is.
pub struct Command<E: Executor> {
    executor: E,
    tenant: String,
}

impl<E: Executor> Command<E> {
    /// The gateway of one tenant: every stream it writes, loads or reads is
    /// that tenant's. `tenant` must not be empty (the default tenant holds
    /// the platform's own streams).
    pub fn new(executor: E, tenant: impl Into<String>) -> Self {
        let tenant = tenant.into();
        debug_assert!(!tenant.is_empty(), "a tenant gateway needs a tenant");
        Self { executor, tenant }
    }

    /// A gateway naming no tenant: for a store that is one tenant's already
    /// (a storefront's connection, which the store API serves in its key's
    /// tenant whatever its requests name). Every call then acts in the
    /// default tenant, which that store maps to its own.
    pub fn unscoped(executor: E) -> Self {
        Self {
            executor,
            tenant: String::new(),
        }
    }

    /// The tenant every call acts in.
    pub fn tenant(&self) -> &str {
        &self.tenant
    }

    /// The store the tenant's streams live in.
    pub fn executor(&self) -> &E {
        &self.executor
    }
}

impl<E: Executor> Deref for Command<E> {
    type Target = E;

    fn deref(&self) -> &Self::Target {
        &self.executor
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
    /// The merchant's public answer, while there is one.
    pub reply: Option<String>,
    /// Unix seconds of the answer as it reads now.
    pub replied_at: Option<u64>,
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
        .handler(on_revised())
        .handler(on_replied())
        .handler(on_reply_removed())
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

#[evento::handler]
async fn on_revised(event: Event<Revised>, review: &mut ReviewState) -> anyhow::Result<()> {
    let data = &event.data;
    review.rating = data.rating;
    review.title = data.title.to_owned();
    review.body = data.body.to_owned();
    review
        .photos
        .retain(|photo| !data.removed.contains(&photo.id));
    review.photos.extend(data.added.iter().cloned());
    review.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_replied(event: Event<Replied>, review: &mut ReviewState) -> anyhow::Result<()> {
    review.reply = Some(event.data.body.to_owned());
    review.replied_at = Some(event.timestamp);
    review.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_reply_removed(
    event: Event<ReplyRemoved>,
    review: &mut ReviewState,
) -> anyhow::Result<()> {
    review.reply = None;
    review.replied_at = None;
    review.updated_at = event.timestamp;
    Ok(())
}

// =============================================================================
// Review requests
// =============================================================================

/// Whether, and when, the shop mails its buyers. The default (no stream) is
/// on, [`DEFAULT_REQUEST_DELAY_DAYS`] after fulfilment.
#[evento::projection(id = id, name = "timada-reviews/ReviewRequestsState")]
#[evento::snapshot(none)]
#[derive(Debug, PartialEq, Eq)]
pub struct ReviewRequestsState {
    pub id: String,
    /// What the merchant set last, if they did.
    pub days: Option<u8>,
    pub disabled: bool,
}

impl ReviewRequestsState {
    /// How many days after fulfilment the shop mails, `None` when it never
    /// does.
    pub fn delay_days(&self) -> Option<u8> {
        if self.disabled {
            return None;
        }
        Some(self.days.unwrap_or(DEFAULT_REQUEST_DELAY_DAYS))
    }
}

pub fn review_requests_projection<E: Executor>() -> Projection<E, ReviewRequestsState> {
    Projection::new::<crate::aggregator::ReviewRequests>()
        .handler(on_request_delay_set())
        .handler(on_requests_disabled())
        .strict()
}

#[evento::handler]
async fn on_request_delay_set(
    event: Event<RequestDelaySet>,
    state: &mut ReviewRequestsState,
) -> anyhow::Result<()> {
    state.id = event.aggregate_id.to_owned();
    state.days = Some(event.data.days);
    state.disabled = false;
    Ok(())
}

#[evento::handler]
async fn on_requests_disabled(
    event: Event<RequestsDisabled>,
    state: &mut ReviewRequestsState,
) -> anyhow::Result<()> {
    state.id = event.aggregate_id.to_owned();
    state.disabled = true;
    Ok(())
}

/// The review request mailed for one order.
#[evento::projection(id = id, name = "timada-reviews/ReviewRequestState")]
#[evento::snapshot(none)]
#[derive(Debug, PartialEq, Eq)]
pub struct ReviewRequestState {
    pub id: String,
    pub order_id: String,
    pub email: String,
    /// Unix seconds.
    pub mailed_at: u64,
}

pub fn review_request_projection<E: Executor>() -> Projection<E, ReviewRequestState> {
    Projection::new::<crate::aggregator::ReviewRequest>()
        .handler(on_request_mailed())
        .strict()
}

#[evento::handler]
async fn on_request_mailed(
    event: Event<RequestMailed>,
    state: &mut ReviewRequestState,
) -> anyhow::Result<()> {
    state.id = event.aggregate_id.to_owned();
    state.order_id = event.data.order_id.clone();
    state.email = event.data.email.clone();
    state.mailed_at = event.timestamp;
    Ok(())
}

/// A mailed link, by the hash of its token.
#[evento::projection(id = id, name = "timada-reviews/ReviewLinkState")]
#[evento::snapshot(none)]
#[derive(Debug, PartialEq, Eq)]
pub struct ReviewLinkState {
    pub id: String,
    pub order_id: String,
    /// Unix seconds.
    pub expires_at: u64,
}

impl ReviewLinkState {
    pub fn is_valid_at(&self, now: u64) -> bool {
        now < self.expires_at
    }
}

pub fn review_link_projection<E: Executor>() -> Projection<E, ReviewLinkState> {
    Projection::new::<crate::aggregator::ReviewLink>()
        .handler(on_link_issued())
        .strict()
}

#[evento::handler]
async fn on_link_issued(
    event: Event<LinkIssued>,
    state: &mut ReviewLinkState,
) -> anyhow::Result<()> {
    state.id = event.aggregate_id.to_owned();
    state.order_id = event.data.order_id.clone();
    state.expires_at = event.data.expires_at;
    Ok(())
}

impl<E: Executor> Command<E> {
    pub async fn load_review_requests(&self) -> anyhow::Result<Option<ReviewRequestsState>> {
        review_requests_projection()
            .load(review_requests_id())
            .tenant(&self.tenant)
            .execute(&self.executor)
            .await
    }

    /// The review request mailed for an order, if one was.
    pub async fn load_review_request(
        &self,
        order_id: &str,
    ) -> anyhow::Result<Option<ReviewRequestState>> {
        review_request_projection()
            .load(review_request_id(order_id))
            .tenant(&self.tenant)
            .execute(&self.executor)
            .await
    }

    /// A link by its stream id ([`review_link_id`](crate::review_link_id) of
    /// the token).
    pub async fn load_review_link(&self, link_id: &str) -> anyhow::Result<Option<ReviewLinkState>> {
        review_link_projection()
            .load(link_id)
            .tenant(&self.tenant)
            .execute(&self.executor)
            .await
    }

    pub async fn load_review(&self, review_id: &str) -> anyhow::Result<Option<ReviewState>> {
        review_projection()
            .load(review_id)
            .tenant(&self.tenant)
            .execute(&self.executor)
            .await
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
