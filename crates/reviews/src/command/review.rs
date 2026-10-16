use evento::{Executor, ProjectionAggregate, WriteError};
use timada_orders::{OrderLine, OrderState, OrderStatus, order_projection};

use crate::aggregator::{PhotoRemoved, Published, Rejected, Revised, Submitted};
use crate::error::ReviewError;
use crate::value_object::{
    MAX_BODY_LEN, MAX_PHOTOS, MAX_RATING, MAX_REASON_LEN, MAX_TITLE_LEN, MIN_BODY_LEN, MIN_RATING,
    ReviewPhoto, ReviewStatus, author_name, non_blank, review_id,
};

use super::ReviewState;

/// What a shopper writes about a product of their order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SubmitReview {
    pub order_id: String,
    pub product_id: String,
    pub rating: u8,
    /// Blank is none.
    pub title: Option<String>,
    pub body: String,
    /// Already stored by whoever took the upload.
    pub photos: Vec<ReviewPhoto>,
}

impl SubmitReview {
    /// Checks what the shopper wrote, without looking at the order: the same
    /// rules [`Command::submit_review`] applies, for an upload to refuse
    /// before it stores any photo.
    pub fn validate(&self) -> Result<(), ReviewError> {
        check_draft(
            self.rating,
            self.title.as_deref(),
            &self.body,
            self.photos.len(),
        )
    }
}

/// What the author of a waiting review changes: the rating, title and text
/// replace the old ones, the photos in `keep` stay and `photos` come after
/// them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReviseReview {
    pub order_id: String,
    pub product_id: String,
    pub rating: u8,
    /// Blank is none.
    pub title: Option<String>,
    pub body: String,
    /// The ids of the photos already on the review that stay on it.
    pub keep: Vec<String>,
    /// New ones, already stored by whoever took the upload.
    pub photos: Vec<ReviewPhoto>,
}

impl ReviseReview {
    /// The rules [`Command::revise_review`] applies to what was written, for
    /// an upload to refuse before it stores any photo.
    pub fn validate(&self) -> Result<(), ReviewError> {
        check_draft(
            self.rating,
            self.title.as_deref(),
            &self.body,
            self.keep.len() + self.photos.len(),
        )
    }
}

/// The rules on what a shopper writes; `photo_count` is how many photos come
/// with it.
pub fn check_draft(
    rating: u8,
    title: Option<&str>,
    body: &str,
    photo_count: usize,
) -> Result<(), ReviewError> {
    if !(MIN_RATING..=MAX_RATING).contains(&rating) {
        return Err(ReviewError::InvalidRating);
    }
    if title.is_some_and(|title| title.trim().chars().count() > MAX_TITLE_LEN) {
        return Err(ReviewError::InvalidTitle);
    }
    let body_len = body.trim().chars().count();
    if !(MIN_BODY_LEN..=MAX_BODY_LEN).contains(&body_len) {
        return Err(ReviewError::InvalidBody);
    }
    if photo_count > MAX_PHOTOS {
        return Err(ReviewError::TooManyPhotos);
    }
    Ok(())
}

impl<E: Executor> super::Command<E> {
    /// The line of a paid, live order a shopper may review `product_id` from,
    /// with the order: refused for an unknown or cancelled order, a product
    /// it does not hold, or one already reviewed from it.
    pub async fn reviewable_line(
        &self,
        order_id: &str,
        product_id: &str,
    ) -> Result<(OrderState, OrderLine), ReviewError> {
        let order = order_projection()
            .load(order_id)
            .execute(&self.0)
            .await?
            .ok_or(ReviewError::OrderNotFound)?;
        if order.status == OrderStatus::Cancelled {
            return Err(ReviewError::OrderCancelled);
        }
        let line = order
            .lines
            .iter()
            .find(|line| line.product_id == product_id)
            .cloned()
            .ok_or(ReviewError::NotPurchased)?;
        if self
            .load_review(&review_id(order_id, product_id))
            .await?
            .is_some()
        {
            return Err(ReviewError::AlreadyReviewed);
        }
        Ok((order, line))
    }
}

#[evento::command]
impl<E: Executor> super::Command<E> {
    /// Records a review waiting for the merchant, signed with the name the
    /// order was shipped to. One per product and order.
    pub async fn submit_review(
        &self,
        actor: &str,
        cmd: SubmitReview,
        routing_key: Option<String>,
    ) -> Result<ReviewState, ReviewError> {
        cmd.validate()?;
        let (order, line) = self.reviewable_line(&cmd.order_id, &cmd.product_id).await?;
        let id = review_id(&order.id, &line.product_id);
        let created = evento::append(&id)
            .original_version(0)
            .routing_key_opt(routing_key)
            .event(&Submitted {
                order_id: order.id.clone(),
                product_id: line.product_id.clone(),
                variant_label: line.variant_label.clone(),
                rating: cmd.rating,
                title: non_blank(cmd.title),
                body: cmd.body.trim().to_owned(),
                author_name: author_name(&order.shipping.name),
                photos: cmd.photos,
                submitted_by: actor.to_owned(),
            })
            .commit(&self.0)
            .await;
        match created {
            Ok(_) => {}
            Err(WriteError::InvalidOriginalVersion) => return Err(ReviewError::AlreadyReviewed),
            Err(err) => return Err(err.into()),
        }
        self.load_review(&id).await?.ok_or(ReviewError::NotFound)
    }

    /// Changes a review while it waits for the merchant; `NotPending` once
    /// it is published or rejected, also when that happens meanwhile. A
    /// revision that changes nothing writes nothing.
    pub async fn revise_review(
        &self,
        actor: &str,
        cmd: ReviseReview,
        routing_key: Option<String>,
    ) -> Result<ReviewState, ReviewError> {
        cmd.validate()?;
        let id = review_id(&cmd.order_id, &cmd.product_id);
        let review = self.load_review(&id).await?.ok_or(ReviewError::NotFound)?;
        if review.status != ReviewStatus::Pending {
            return Err(ReviewError::NotPending);
        }
        if cmd
            .keep
            .iter()
            .any(|kept| !review.photos.iter().any(|photo| &photo.id == kept))
        {
            return Err(ReviewError::PhotoNotFound);
        }
        let title = non_blank(cmd.title);
        let body = cmd.body.trim().to_owned();
        let removed: Vec<String> = review
            .photos
            .iter()
            .filter(|photo| !cmd.keep.contains(&photo.id))
            .map(|photo| photo.id.clone())
            .collect();
        if cmd.rating == review.rating
            && title == review.title
            && body == review.body
            && removed.is_empty()
            && cmd.photos.is_empty()
        {
            return Ok(review);
        }
        let revised = review
            .write()?
            .routing_key_opt(routing_key)
            .event(&Revised {
                rating: cmd.rating,
                title,
                body,
                added: cmd.photos,
                removed,
                revised_by: actor.to_owned(),
            })
            .commit(&self.0)
            .await;
        match revised {
            Ok(_) => {}
            Err(WriteError::InvalidOriginalVersion) => return Err(ReviewError::NotPending),
            Err(err) => return Err(err.into()),
        }
        self.load_review(&id).await?.ok_or(ReviewError::NotFound)
    }

    /// Shows it on the shop; already published is a no-op.
    pub async fn publish_review(
        &self,
        actor: &str,
        review_id: &str,
        routing_key: Option<String>,
    ) -> Result<(), ReviewError> {
        let review = self.require_open(review_id).await?;
        if review.status == ReviewStatus::Published {
            return Ok(());
        }
        review
            .write()?
            .routing_key_opt(routing_key)
            .event(&Published {
                published_by: actor.to_owned(),
            })
            .commit(&self.0)
            .await?;
        Ok(())
    }

    /// Hides it for good, waiting or published; already rejected is a no-op.
    pub async fn reject_review(
        &self,
        actor: &str,
        review_id: &str,
        reason: &str,
        routing_key: Option<String>,
    ) -> Result<(), ReviewError> {
        let reason = reason.trim();
        if reason.chars().count() > MAX_REASON_LEN {
            return Err(ReviewError::InvalidReason);
        }
        let review = match self.require_open(review_id).await {
            Ok(review) => review,
            Err(ReviewError::Rejected) => return Ok(()),
            Err(err) => return Err(err),
        };
        review
            .write()?
            .routing_key_opt(routing_key)
            .event(&Rejected {
                reason: reason.to_owned(),
                rejected_by: actor.to_owned(),
            })
            .commit(&self.0)
            .await?;
        Ok(())
    }

    /// Takes one photo off; one already gone is `PhotoNotFound`.
    pub async fn remove_review_photo(
        &self,
        actor: &str,
        review_id: &str,
        image_id: &str,
        routing_key: Option<String>,
    ) -> Result<(), ReviewError> {
        let review = self.require_open(review_id).await?;
        if !review.photos.iter().any(|photo| photo.id == image_id) {
            return Err(ReviewError::PhotoNotFound);
        }
        review
            .write()?
            .routing_key_opt(routing_key)
            .event(&PhotoRemoved {
                image_id: image_id.to_owned(),
                removed_by: actor.to_owned(),
            })
            .commit(&self.0)
            .await?;
        Ok(())
    }
}
