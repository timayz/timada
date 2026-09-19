use evento::{Executor, ProjectionAggregate};

use crate::{aggregator::ReturnRefused, error::ReturnError, value_object::ReturnStatus};

impl<E: Executor> super::Command<'_, E> {
    /// Turns the request down and frees the units it was holding.
    pub async fn refuse_return(
        &self,
        id: impl Into<String>,
        reason: impl Into<String>,
    ) -> Result<(), ReturnError> {
        let reason = reason.into();
        if reason.trim().is_empty() {
            return Err(ReturnError::Required("reason"));
        }
        let request = self.load_existing(id).await?;
        if request.status == ReturnStatus::Refused {
            return self.release_claims(&request.id).await;
        }
        request.expect_status(ReturnStatus::Requested)?;

        request
            .write()?
            .event(&ReturnRefused {
                reason: reason.trim().to_owned(),
            })
            .commit(self.executor)
            .await?;
        self.release_claims(&request.id).await?;
        tracing::info!(return_id = %request.id, "return refused");
        Ok(())
    }
}
