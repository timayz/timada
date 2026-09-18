use evento::{Executor, ProjectionAggregate};

use crate::{aggregator::ReviewRejected, error::ReviewError};

impl<E: Executor> super::Command<E> {
    pub async fn reject_review(
        &self,
        id: impl Into<String>,
        reason: String,
    ) -> Result<(), ReviewError> {
        if reason.trim().is_empty() {
            return Err(ReviewError::Required("reason"));
        }
        let review = self.load_pending(id).await?;

        review
            .write()?
            .event(&ReviewRejected { reason })
            .commit(&self.0)
            .await?;
        tracing::info!(review_id = %review.id, "review rejected");
        Ok(())
    }
}
