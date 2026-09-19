use evento::{Executor, ProjectionAggregate};
use timada_core::Money;

use crate::{aggregator::VoucherRedemptionRefunded, error::PromotionError};

use super::voucher_id;

impl<E: Executor> super::Command<'_, E> {
    /// Puts back what an order spent on a voucher (the order fell through).
    /// Returns the refunded amount, `None` when the order spent nothing —
    /// which makes a repeat a no-op.
    pub async fn refund_voucher(
        &self,
        code: &str,
        order_id: &str,
    ) -> Result<Option<Money>, PromotionError> {
        let Some(voucher) = self.load_voucher(voucher_id(code)).await? else {
            return Err(PromotionError::UnknownCode);
        };
        let Some(amount) = voucher.redemption_for(order_id).map(|r| r.amount.clone()) else {
            return Ok(None);
        };

        voucher
            .write()?
            .event(&VoucherRedemptionRefunded {
                order_id: order_id.to_owned(),
                amount: amount.clone(),
            })
            .commit(self.executor)
            .await?;
        tracing::info!(voucher_id = %voucher.id, %order_id, "voucher redemption refunded");
        Ok(Some(amount))
    }
}
