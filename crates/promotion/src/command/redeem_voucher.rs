use evento::{Executor, ProjectionAggregate};
use timada_core::Money;

use crate::{aggregator::VoucherRedeemed, error::PromotionError};

use super::{VoucherState, voucher_id};

impl<E: Executor> super::Command<'_, E> {
    /// Spends part of a voucher on an order and returns the remaining balance.
    /// A repeat for the same order spends nothing more.
    pub async fn redeem_voucher(
        &self,
        code: &str,
        order_id: impl Into<String>,
        amount: Money,
    ) -> Result<Money, PromotionError> {
        let order_id = order_id.into();
        if !amount.is_positive() {
            return Err(PromotionError::InvalidAmount);
        }
        let voucher = self.load_voucher_for(code, &order_id).await?;
        if voucher.redemption_for(&order_id).is_some() {
            return Ok(voucher.remaining);
        }
        usable(&voucher)?;
        let remaining = voucher.remaining.checked_sub(&amount)?;
        if remaining.is_negative() {
            return Err(PromotionError::InsufficientBalance {
                remaining: voucher.remaining.clone(),
            });
        }

        self.write_redemption(&voucher, order_id, amount).await?;
        Ok(remaining)
    }

    /// Spends as much of a voucher as an order can take — its balance, capped
    /// at `max` — and returns the amount spent. A repeat for the same order
    /// returns what was spent the first time.
    pub async fn spend_voucher(
        &self,
        code: &str,
        order_id: impl Into<String>,
        max: &Money,
    ) -> Result<Money, PromotionError> {
        let order_id = order_id.into();
        let voucher = self.load_voucher_for(code, &order_id).await?;
        if let Some(done) = voucher.redemption_for(&order_id) {
            return Ok(done.amount.clone());
        }
        usable(&voucher)?;
        voucher.remaining.same_currency(max)?;
        if !max.is_positive() {
            return Err(PromotionError::NotApplicable);
        }
        if !voucher.remaining.is_positive() {
            return Err(PromotionError::InsufficientBalance {
                remaining: voucher.remaining.clone(),
            });
        }
        let amount = Money::new(voucher.remaining.minor.min(max.minor), &max.currency);

        self.write_redemption(&voucher, order_id, amount.clone())
            .await?;
        Ok(amount)
    }

    async fn load_voucher_for(
        &self,
        code: &str,
        order_id: &str,
    ) -> Result<VoucherState, PromotionError> {
        if order_id.trim().is_empty() {
            return Err(PromotionError::Required("order_id"));
        }
        self.load_voucher(voucher_id(code))
            .await?
            .ok_or(PromotionError::UnknownCode)
    }

    async fn write_redemption(
        &self,
        voucher: &VoucherState,
        order_id: String,
        amount: Money,
    ) -> Result<(), PromotionError> {
        voucher
            .write()?
            .event(&VoucherRedeemed {
                order_id: order_id.clone(),
                amount,
            })
            .commit(self.executor)
            .await?;
        tracing::info!(voucher_id = %voucher.id, %order_id, "voucher redeemed");
        Ok(())
    }
}

fn usable(voucher: &VoucherState) -> Result<(), PromotionError> {
    if voucher.cancelled {
        return Err(PromotionError::Cancelled);
    }
    if let Some(until) = voucher.expires_at
        && until < timada_core::time::now_unix_secs()?
    {
        return Err(PromotionError::Expired);
    }
    Ok(())
}
