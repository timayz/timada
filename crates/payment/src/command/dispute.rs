use evento::{Executor, ProjectionAggregate};
use timada_core::Money;

use crate::{
    aggregator::{DisputeLost, DisputeOpened, DisputeWon, RefundFailed},
    error::PaymentError,
    value_object::{DisputeStatus, PaymentStatus},
};

/// A dispute as the provider reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenDispute {
    /// The provider's own reference for the dispute.
    pub dispute_id: String,
    pub amount: Money,
    /// The provider's reason code, kept as given (see
    /// [`crate::dispute_reason_label`]).
    pub reason: String,
    /// Unix seconds: when the shop's evidence is due.
    pub respond_by: Option<u64>,
}

/// Why refunds that no longer fit are failed when a dispute is lost.
pub const DISPUTE_LOST: &str = "dispute lost";

impl<E: Executor> super::Command<'_, E> {
    /// Records that the cardholder contested the charge. Only money that was
    /// captured can be disputed. Returns `false` when the dispute is known
    /// already — a provider reports each one several times.
    pub async fn open_dispute(
        &self,
        id: impl Into<String>,
        dispute: OpenDispute,
    ) -> Result<bool, PaymentError> {
        let payment = self.load_existing(id).await?;
        if dispute.dispute_id.trim().is_empty() {
            return Err(PaymentError::DisputeReferenceRequired);
        }
        if payment
            .disputes
            .iter()
            .any(|d| d.dispute_id == dispute.dispute_id)
        {
            return Ok(false);
        }
        if !matches!(
            payment.status,
            PaymentStatus::Captured | PaymentStatus::Refunded
        ) {
            return Err(PaymentError::NotCaptured);
        }
        if !dispute.amount.is_positive() {
            return Err(PaymentError::InvalidAmount);
        }
        if dispute.amount.minor > payment.amount.minor {
            return Err(PaymentError::DisputeExceedsCapture);
        }

        payment
            .write()?
            .event(&DisputeOpened {
                dispute_id: dispute.dispute_id.clone(),
                amount: dispute.amount,
                reason: dispute.reason,
                respond_by: dispute.respond_by,
            })
            .commit(self.0)
            .await?;
        tracing::warn!(payment_id = %payment.id, dispute_id = %dispute.dispute_id, "payment disputed");
        Ok(true)
    }

    /// The bank sided with the shop. Returns `false` when it was recorded
    /// already.
    pub async fn win_dispute(
        &self,
        id: impl Into<String>,
        dispute_id: &str,
    ) -> Result<bool, PaymentError> {
        let payment = self.load_existing(id).await?;
        match status_of(&payment, dispute_id)? {
            DisputeStatus::Won => return Ok(false),
            DisputeStatus::Lost => return Err(PaymentError::DisputeAlreadyClosed),
            DisputeStatus::Open => {}
        }
        payment
            .write()?
            .event(&DisputeWon {
                dispute_id: dispute_id.to_owned(),
            })
            .commit(self.0)
            .await?;
        tracing::info!(payment_id = %payment.id, %dispute_id, "dispute won");
        Ok(true)
    }

    /// The bank sided with the cardholder: the disputed amount is gone.
    /// Refunds that were waiting for the dispute and no longer fit in what is
    /// left fail with it, oldest kept first. Returns `false` when it was
    /// recorded already.
    pub async fn lose_dispute(
        &self,
        id: impl Into<String>,
        dispute_id: &str,
    ) -> Result<bool, PaymentError> {
        let payment = self.load_existing(id).await?;
        match status_of(&payment, dispute_id)? {
            DisputeStatus::Lost => return Ok(false),
            DisputeStatus::Won => return Err(PaymentError::DisputeAlreadyClosed),
            DisputeStatus::Open => {}
        }
        let lost = payment
            .disputes
            .iter()
            .find(|d| d.dispute_id == dispute_id)
            .map(|d| d.amount.clone())
            .ok_or(PaymentError::DisputeNotFound)?;

        // What the pending refunds may still share.
        let mut left = payment
            .amount
            .checked_sub(&payment.refunded)?
            .checked_sub(&payment.charged_back()?)?
            .checked_sub(&lost)?
            .minor;
        let mut write = payment.write()?;
        write.event(&DisputeLost {
            dispute_id: dispute_id.to_owned(),
        });
        for refund in &payment.pending_refunds {
            if refund.amount.minor <= left {
                left -= refund.amount.minor;
            } else {
                write.event(&RefundFailed {
                    refund_id: refund.refund_id.clone(),
                    reason: DISPUTE_LOST.to_owned(),
                });
            }
        }
        write.commit(self.0).await?;
        tracing::warn!(payment_id = %payment.id, %dispute_id, "dispute lost");
        Ok(true)
    }
}

fn status_of(
    payment: &super::PaymentState,
    dispute_id: &str,
) -> Result<DisputeStatus, PaymentError> {
    payment
        .disputes
        .iter()
        .find(|d| d.dispute_id == dispute_id)
        .map(|d| d.status)
        .ok_or(PaymentError::DisputeNotFound)
}
