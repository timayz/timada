//! What a provider tells the shop on its own — a webhook, once the host
//! checked its signature and turned it into a [`ProviderEvent`].

use evento::Executor;
use sqlx::SqlitePool;
use timada_core::Money;

use crate::{
    command::{Command, OpenDispute},
    dispute_list::payment_by_reference,
    error::PaymentError,
    provider::{PaymentProvider, ProviderRefund},
    query::load_payment,
    refund_execution::refund_by_provider_reference,
    value_object::PaymentStatus,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderEvent {
    /// The shopper paid. A payment that *failed* is no event: the shopper may
    /// try again on the same session, and only the timeout declines.
    Paid {
        payment_id: String,
        reference: String,
        amount: Money,
    },
    /// A refund the provider had taken as pending went through.
    RefundSettled { provider_reference: String },
    /// A refund the provider had taken as pending did not go through.
    RefundFailed {
        provider_reference: String,
        reason: String,
    },
    /// Where a dispute stands. A provider reports the same dispute many times
    /// — opened, evidence updated, funds moved, closed — and not always in
    /// order: each report says everything, so any of them can be the first.
    Dispute(ProviderDispute),
}

/// A dispute as the provider reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderDispute {
    /// What the disputed payment was captured under (`PaymentCaptured`'s
    /// `psp_reference`).
    pub payment_reference: String,
    /// The provider's own reference for the dispute.
    pub reference: String,
    pub amount: Money,
    /// The provider's reason code, as given.
    pub reason: String,
    /// Unix seconds: when the shop's evidence is due.
    pub respond_by: Option<u64>,
    pub standing: DisputeStanding,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisputeStanding {
    Open,
    Won,
    Lost,
}

/// What [`apply_provider_event`] did with an event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Applied {
    Done,
    /// Seen before, or about something this shop does not know: nothing to do.
    Ignored,
    /// Money arrived for a payment that can no longer take it (declined by the
    /// timeout, or the wrong amount): it was sent straight back.
    SentBack,
}

/// Records a provider's event in the payment's stream. Safe to repeat — a
/// provider delivers its events at least once.
pub async fn apply_provider_event<E: Executor>(
    executor: &E,
    db: &SqlitePool,
    provider: &dyn PaymentProvider,
    event: ProviderEvent,
) -> Result<Applied, PaymentError> {
    let cmd = Command(executor);
    match event {
        ProviderEvent::Paid {
            payment_id,
            reference,
            amount,
        } => {
            let Some(payment) = load_payment(executor, &payment_id).await? else {
                tracing::warn!(%payment_id, "provider reports an unknown payment as paid");
                return Ok(Applied::Ignored);
            };
            if payment.psp_reference.as_deref() == Some(&reference) {
                return Ok(Applied::Ignored);
            }
            if payment.status == PaymentStatus::Requested && payment.amount == amount {
                return match cmd.capture_payment(&payment_id, reference.clone()).await {
                    Ok(()) => Ok(Applied::Done),
                    // Captured or declined while we were looking: look again.
                    Err(PaymentError::NotRequested) => {
                        Box::pin(apply_provider_event(
                            executor,
                            db,
                            provider,
                            ProviderEvent::Paid {
                                payment_id,
                                reference,
                                amount,
                            },
                        ))
                        .await
                    }
                    Err(err) => Err(err),
                };
            }

            // The order is gone (or the amount is not the one asked for): the
            // shop must not keep this money.
            tracing::error!(%payment_id, %reference, status = ?payment.status, "payment received but not expected: sending it back");
            provider
                .refund(&ProviderRefund {
                    psp_reference: reference.clone(),
                    amount,
                    idempotency_key: format!("unexpected-{reference}"),
                })
                .await?;
            Ok(Applied::SentBack)
        }
        ProviderEvent::RefundSettled { provider_reference } => {
            let Some((payment_id, refund_id)) =
                refund_by_provider_reference(db, &provider_reference).await?
            else {
                return Ok(Applied::Ignored);
            };
            let settled = cmd
                .settle_refund(&payment_id, &refund_id, provider_reference)
                .await?;
            Ok(if settled {
                Applied::Done
            } else {
                Applied::Ignored
            })
        }
        ProviderEvent::RefundFailed {
            provider_reference,
            reason,
        } => {
            let Some((payment_id, refund_id)) =
                refund_by_provider_reference(db, &provider_reference).await?
            else {
                return Ok(Applied::Ignored);
            };
            match cmd.fail_refund(&payment_id, &refund_id, reason).await {
                Ok(true) => Ok(Applied::Done),
                Ok(false) | Err(PaymentError::RefundAlreadySettled) => Ok(Applied::Ignored),
                Err(err) => Err(err),
            }
        }
        ProviderEvent::Dispute(dispute) => {
            // The reference is learnt by the `payment-dispute-list`
            // subscription; a dispute comes days after the capture.
            let Some(payment_id) = payment_by_reference(db, &dispute.payment_reference).await?
            else {
                tracing::warn!(
                    dispute = %dispute.reference,
                    payment_reference = %dispute.payment_reference,
                    "provider reports a dispute on a payment this shop does not know"
                );
                return Ok(Applied::Ignored);
            };
            let opened = cmd
                .open_dispute(
                    &payment_id,
                    OpenDispute {
                        dispute_id: dispute.reference.clone(),
                        amount: dispute.amount,
                        reason: dispute.reason,
                        respond_by: dispute.respond_by,
                    },
                )
                .await?;
            let closed = match dispute.standing {
                DisputeStanding::Open => false,
                DisputeStanding::Won => cmd.win_dispute(&payment_id, &dispute.reference).await?,
                DisputeStanding::Lost => cmd.lose_dispute(&payment_id, &dispute.reference).await?,
            };
            Ok(if opened || closed {
                Applied::Done
            } else {
                Applied::Ignored
            })
        }
    }
}
