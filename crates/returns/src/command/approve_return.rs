use evento::{Executor, ProjectionAggregate};

use crate::{aggregator::ReturnApproved, error::ReturnError, value_object::ReturnStatus};

impl<E: Executor> super::Command<'_, E> {
    /// Accepts the request: the customer may ship the parcel. A no-op when
    /// already approved.
    pub async fn approve_return(&self, id: impl Into<String>) -> Result<(), ReturnError> {
        let request = self.load_existing(id).await?;
        if request.status == ReturnStatus::Approved {
            return Ok(());
        }
        request.expect_status(ReturnStatus::Requested)?;

        request
            .write()?
            .event(&ReturnApproved)
            .commit(self.executor)
            .await?;
        tracing::info!(return_id = %request.id, "return approved");
        Ok(())
    }
}
