use evento::{Executor, ProjectionAggregate};

use crate::{aggregator::ReturnCancelled, error::ReturnError, value_object::ReturnStatus};

impl<E: Executor> super::Command<'_, E> {
    /// The customer withdraws a return the shop has not received yet, which
    /// frees the units it was holding. Someone else's return is reported as
    /// not found.
    pub async fn cancel_return(
        &self,
        id: impl Into<String>,
        customer_id: &str,
    ) -> Result<(), ReturnError> {
        let request = self.load_existing(id).await?;
        if request.customer_id != customer_id {
            return Err(ReturnError::ReturnNotFound);
        }
        if request.status == ReturnStatus::Cancelled {
            return self.release_claims(&request.id).await;
        }
        if !request.status.is_open() {
            return Err(ReturnError::WrongStatus {
                expected: "requested or approved",
                actual: request.status.as_str(),
            });
        }

        request
            .write()?
            .event(&ReturnCancelled)
            .commit(self.executor)
            .await?;
        self.release_claims(&request.id).await?;
        tracing::info!(return_id = %request.id, "return cancelled");
        Ok(())
    }
}
