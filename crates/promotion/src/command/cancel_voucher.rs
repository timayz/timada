use evento::{Executor, ProjectionAggregate};

use crate::{aggregator::VoucherCancelled, error::PromotionError};

impl<E: Executor> super::Command<'_, E> {
    /// Voids the remaining balance. A no-op when already cancelled.
    pub async fn cancel_voucher(
        &self,
        id: impl Into<String>,
        reason: impl Into<String>,
    ) -> Result<(), PromotionError> {
        let Some(voucher) = self.load_voucher(id).await? else {
            return Err(PromotionError::UnknownCode);
        };
        if voucher.cancelled {
            return Ok(());
        }

        voucher
            .write()?
            .event(&VoucherCancelled {
                reason: reason.into(),
            })
            .commit(self.executor)
            .await?;
        tracing::info!(voucher_id = %voucher.id, "voucher cancelled");
        Ok(())
    }
}
