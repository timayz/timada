use evento::{Executor, ProjectionAggregate};
use timada_core::Money;

use crate::{
    aggregator::{PaymentRefunded, RefundFailed, RefundRequested, RefundSettled},
    error::PaymentError,
    value_object::PaymentStatus,
};

/// Deterministic refund id: the `nth` refund asked of a payment.
fn refund_id(payment_id: &str, nth: u32) -> String {
    timada_core::id::derived(&[payment_id, &nth.to_string()], "refund")
}

impl<E: Executor> super::Command<'_, E> {
    /// Asks for part or all of a captured payment to be given back; several
    /// partial refunds may follow each other up to the captured amount, which
    /// pending refunds count against.
    ///
    /// Nothing has moved yet: the refund is *requested*, and becomes a
    /// `PaymentRefunded` once the provider confirmed it
    /// ([`Self::settle_refund`]). Returns the refund's id.
    pub async fn refund_payment(
        &self,
        id: impl Into<String>,
        amount: Money,
        reason: String,
    ) -> Result<String, PaymentError> {
        let payment = self.load_existing(id).await?;
        if payment.status != PaymentStatus::Captured {
            return Err(PaymentError::NotCaptured);
        }
        if !amount.is_positive() {
            return Err(PaymentError::InvalidAmount);
        }
        if !payment.can_refund(&amount)? {
            return Err(PaymentError::RefundExceedsCapture);
        }

        let refund_id = refund_id(&payment.id, payment.refund_requests + 1);
        payment
            .write()?
            .event(&RefundRequested {
                refund_id: refund_id.clone(),
                amount,
                reason,
            })
            .commit(self.0)
            .await?;
        tracing::info!(payment_id = %payment.id, %refund_id, "refund requested");
        Ok(refund_id)
    }

    /// [`Self::refund_payment`] for process managers: `reference` (say, a
    /// return's number) is recorded as the refund's reason and is its
    /// idempotency key — a payment already asked to refund that reference is
    /// left alone, whatever became of the refund, so a retry after a crash
    /// never refunds twice. Returns whether a refund was asked for.
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

    /// The provider gave the money back: the refund becomes a
    /// `PaymentRefunded`, with a `RefundSettled` naming the request and the
    /// provider's reference. A refund that failed can be settled too — the
    /// operator returned the money some other way — as long as the captured
    /// amount still covers it. Returns `false` when it was settled already.
    pub async fn settle_refund(
        &self,
        id: impl Into<String>,
        refund_id: &str,
        psp_refund_reference: String,
    ) -> Result<bool, PaymentError> {
        let payment = self.load_existing(id).await?;
        if payment.settled_refund_ids.iter().any(|r| r == refund_id) {
            return Ok(false);
        }
        let pending = payment
            .pending_refunds
            .iter()
            .find(|r| r.refund_id == refund_id);
        let refund = match pending {
            Some(refund) => refund.clone(),
            None => {
                let failed = payment
                    .failed_refunds
                    .iter()
                    .find(|r| r.refund_id == refund_id)
                    .ok_or(PaymentError::RefundNotFound)?;
                if !payment.can_refund(&failed.amount)? {
                    return Err(PaymentError::RefundExceedsCapture);
                }
                failed.clone()
            }
        };

        payment
            .write()?
            .event(&PaymentRefunded {
                amount: refund.amount,
                reason: refund.reason,
            })
            .event(&RefundSettled {
                refund_id: refund_id.to_owned(),
                psp_refund_reference,
            })
            .commit(self.0)
            .await?;
        tracing::info!(payment_id = %payment.id, %refund_id, "refund settled");
        Ok(true)
    }

    /// The provider refused the refund for good: what it held is released.
    /// Returns `false` when it had failed already.
    pub async fn fail_refund(
        &self,
        id: impl Into<String>,
        refund_id: &str,
        reason: String,
    ) -> Result<bool, PaymentError> {
        let payment = self.load_existing(id).await?;
        if payment.settled_refund_ids.iter().any(|r| r == refund_id) {
            return Err(PaymentError::RefundAlreadySettled);
        }
        if payment
            .failed_refunds
            .iter()
            .any(|r| r.refund_id == refund_id)
        {
            return Ok(false);
        }
        if !payment
            .pending_refunds
            .iter()
            .any(|r| r.refund_id == refund_id)
        {
            return Err(PaymentError::RefundNotFound);
        }

        payment
            .write()?
            .event(&RefundFailed {
                refund_id: refund_id.to_owned(),
                reason,
            })
            .commit(self.0)
            .await?;
        tracing::warn!(payment_id = %payment.id, %refund_id, "refund failed");
        Ok(true)
    }

    /// Asks again for a refund that failed: the same refund goes back to
    /// pending, provided the captured amount still covers it.
    pub async fn retry_refund(
        &self,
        id: impl Into<String>,
        refund_id: &str,
    ) -> Result<(), PaymentError> {
        let payment = self.load_existing(id).await?;
        let Some(failed) = payment
            .failed_refunds
            .iter()
            .find(|r| r.refund_id == refund_id)
        else {
            let known = payment.settled_refund_ids.iter().any(|r| r == refund_id)
                || payment
                    .pending_refunds
                    .iter()
                    .any(|r| r.refund_id == refund_id);
            return Err(if known {
                PaymentError::RefundNotFailed
            } else {
                PaymentError::RefundNotFound
            });
        };
        if !payment.can_refund(&failed.amount)? {
            return Err(PaymentError::RefundExceedsCapture);
        }

        let again = RefundRequested {
            refund_id: failed.refund_id.clone(),
            amount: failed.amount.clone(),
            reason: failed.reason.clone(),
        };
        payment.write()?.event(&again).commit(self.0).await?;
        tracing::info!(payment_id = %payment.id, %refund_id, "failed refund requested again");
        Ok(())
    }
}
