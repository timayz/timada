use evento::{Executor, ProjectionAggregate};
use timada_core::Money;

use crate::{
    aggregator::{ReplacementAbandoned, ReplacementArranged, ReturnCompleted},
    error::ReturnError,
    value_object::{ReplacementStatus, ReturnStatus},
};

impl<E: Executor> super::Command<'_, E> {
    /// Closes a received return once the process manager restocked and
    /// refunded — or arranged the replacement, whose parcel is then named by
    /// `replacement_shipment`. A no-op when already completed.
    pub(crate) async fn complete_return(
        &self,
        id: impl Into<String>,
        refunded: Money,
        credited: Money,
        voucher_code: Option<String>,
        replacement_shipment: Option<String>,
    ) -> Result<(), ReturnError> {
        let request = self.load_existing(id).await?;
        if request.status == ReturnStatus::Completed {
            return Ok(());
        }
        request.expect_status(ReturnStatus::Received)?;

        let mut write = request.write()?;
        write.event(&ReturnCompleted {
            refunded,
            credited,
            voucher_code,
        });
        if let Some(shipment_id) = replacement_shipment {
            write.event(&ReplacementArranged { shipment_id });
        }
        write.commit(self.executor).await?;
        tracing::info!(return_id = %request.id, "return completed");
        Ok(())
    }

    /// The replacement cannot be sent after all: the return goes on as a
    /// refund of the fallback amounts. A no-op when recorded already.
    pub(crate) async fn abandon_replacement(
        &self,
        id: impl Into<String>,
        reason: String,
    ) -> Result<(), ReturnError> {
        let request = self.load_existing(id).await?;
        if request.replacement != Some(ReplacementStatus::Planned) {
            return Ok(());
        }
        request.expect_status(ReturnStatus::Received)?;
        request
            .write()?
            .event(&ReplacementAbandoned {
                reason: reason.clone(),
            })
            .commit(self.executor)
            .await?;
        tracing::warn!(return_id = %request.id, %reason, "replacement abandoned: refunding instead");
        Ok(())
    }
}
