use evento::{Executor, ProjectionAggregate};

use crate::{aggregator::ReviewPublished, error::ReviewError};

impl<E: Executor> super::Command<'_, E> {
    pub async fn publish_review(&self, id: impl Into<String>) -> Result<(), ReviewError> {
        let review = self.load_pending(id).await?;

        review
            .write()?
            .event(&ReviewPublished)
            .commit(self.0)
            .await?;
        tracing::info!(review_id = %review.id, "review published");
        Ok(())
    }
}
