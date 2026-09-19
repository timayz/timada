use evento::{Executor, ProjectionAggregate};
use timada_core::Money;

use crate::{aggregator::PaymentRefunded, error::PaymentError, value_object::PaymentStatus};

impl<E: Executor> super::Command<'_, E> {
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
            .commit(self.0)
            .await?;
        Ok(())
    }

    /// [`Self::refund_payment`] for process managers: `reference` (say, a
    /// return's number) is recorded as the refund's reason and is its
    /// idempotency key — a payment already refunded for that reference is left
    /// alone, so a retry after a crash never refunds twice. Returns whether a
    /// refund was made.
    pub async fn refund_payment_once(
        &self,
        id: impl Into<String>,
        reference: impl Into<String>,
        amount: Money,
    ) -> Result<bool, PaymentError> {
        let (id, reference) = (id.into(), reference.into());
        let payment = self.load_existing(&id).await?;
        if payment.refund_reasons.contains(&reference) {
            return Ok(false);
        }
        self.refund_payment(id, amount, reference).await?;
        Ok(true)
    }
}
