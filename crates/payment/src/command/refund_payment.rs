use evento::{Executor, ProjectionAggregate};
use timada_core::Money;

use crate::{aggregator::PaymentRefunded, error::PaymentError, value_object::PaymentStatus};

impl<E: Executor> super::Command<E> {
    /// Refunds part or all of a captured payment; several partial refunds may
    /// follow each other up to the captured amount.
    pub async fn refund_payment(
        &self,
        id: impl Into<String>,
        amount: Money,
        reason: String,
    ) -> Result<(), PaymentError> {
        let payment = self.load_existing(id).await?;
        if payment.status != PaymentStatus::Captured {
            return Err(PaymentError::NotCaptured);
        }
        if !amount.is_positive() {
            return Err(PaymentError::InvalidAmount);
        }
        let refunded = payment.refunded.checked_add(&amount)?;
        if refunded.minor > payment.amount.minor {
            return Err(PaymentError::RefundExceedsCapture);
        }

        payment
            .write()?
            .event(&PaymentRefunded { amount, reason })
            .commit(&self.0)
            .await?;
        Ok(())
    }
}
