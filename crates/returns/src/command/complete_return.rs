use evento::{Executor, ProjectionAggregate};
use timada_core::Money;

use crate::{aggregator::ReturnCompleted, error::ReturnError, value_object::ReturnStatus};

impl<E: Executor> super::Command<'_, E> {
    /// Closes a received return once the process manager restocked and
    /// refunded. A no-op when already completed.
    pub(crate) async fn complete_return(
        &self,
        id: impl Into<String>,
        refunded: Money,
        credited: Money,
        voucher_code: Option<String>,
    ) -> Result<(), ReturnError> {
        let request = self.load_existing(id).await?;
        if request.status == ReturnStatus::Completed {
            return Ok(());
        }
        request.expect_status(ReturnStatus::Received)?;

        request
            .write()?
            .event(&ReturnCompleted {
                refunded,
                credited,
                voucher_code,
            })
            .commit(self.executor)
            .await?;
        tracing::info!(return_id = %request.id, "return completed");
        Ok(())
    }
}
