use evento::{Executor, ProjectionAggregate};
use timada_core::Money;

use crate::{aggregator::VoucherRedeemed, error::PromotionError};

use super::voucher_id;

impl<E: Executor> super::Command<'_, E> {
    /// Spends part of a voucher on an order and returns the remaining balance.
    pub async fn redeem_voucher(
        &self,
        code: &str,
        order_id: impl Into<String>,
        amount: Money,
    ) -> Result<Money, PromotionError> {
        let order_id = order_id.into();
        if order_id.trim().is_empty() {
            return Err(PromotionError::Required("order_id"));
        }
        if !amount.is_positive() {
            return Err(PromotionError::InvalidAmount);
        }
        let Some(voucher) = self.load_voucher(voucher_id(code)).await? else {
            return Err(PromotionError::UnknownCode);
        };
        if voucher.cancelled {
            return Err(PromotionError::Cancelled);
        }
        if let Some(until) = voucher.expires_at
            && until < timada_core::time::now_unix_secs()?
        {
            return Err(PromotionError::Expired);
        }
        let remaining = voucher.remaining.checked_sub(&amount)?;
        if remaining.is_negative() {
            return Err(PromotionError::InsufficientBalance {
                remaining: voucher.remaining.clone(),
            });
        }

        voucher
            .write()?
            .event(&VoucherRedeemed {
                order_id: order_id.clone(),
                amount,
            })
            .commit(self.executor)
            .await?;
        tracing::info!(voucher_id = %voucher.id, %order_id, "voucher redeemed");
        Ok(remaining)
    }
}
