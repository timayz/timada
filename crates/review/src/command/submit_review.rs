use evento::Executor;

use crate::{aggregator::ReviewSubmitted, error::ReviewError};

use super::review_id;

#[derive(Debug, Clone)]
pub struct SubmitReview {
    pub product_id: String,
    pub customer_id: String,
    pub order_id: Option<String>,
    pub rating: u8,
    pub title: String,
    pub body: String,
}

#[evento::command]
impl<E: Executor> super::Command<'_, E> {
    /// Leaves a review. The id is derived from (product, customer), so a
    /// second review by the same customer is rejected atomically by the store.
    pub async fn submit_review(
        &self,
        cmd: SubmitReview,
        routing_key: Option<String>,
    ) -> Result<String, ReviewError> {
        if cmd.product_id.trim().is_empty() {
            return Err(ReviewError::Required("product_id"));
        }
        if cmd.customer_id.trim().is_empty() {
            return Err(ReviewError::Required("customer_id"));
        }
        if !(1..=5).contains(&cmd.rating) {
            return Err(ReviewError::InvalidRating(cmd.rating));
        }
        if cmd.body.trim().is_empty() {
            return Err(ReviewError::Required("body"));
        }

        let id = review_id(&cmd.product_id, &cmd.customer_id);
        let result = evento::append(&id)
            .routing_key_opt(routing_key)
            .event(&ReviewSubmitted {
                product_id: cmd.product_id,
                customer_id: cmd.customer_id,
                order_id: cmd.order_id,
                rating: cmd.rating,
                title: cmd.title,
                body: cmd.body,
            })
            .commit(self.0)
            .await;

        match result {
            Ok(id) => {
                tracing::info!(review_id = %id, "review submitted");
                Ok(id)
            }
            Err(evento::WriteError::InvalidOriginalVersion) => Err(ReviewError::AlreadyReviewed),
            Err(err) => Err(err.into()),
        }
    }
}
