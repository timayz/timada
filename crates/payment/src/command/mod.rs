mod capture_payment;
mod decline_payment;
mod dispute;
mod refund_payment;
mod request_payment;

use std::ops::Deref;

pub use dispute::OpenDispute;
pub use request_payment::RequestPayment;

use evento::{Executor, Projection, metadata::Event};
use timada_core::Money;

use crate::{
    aggregator::{
        DisputeLost, DisputeOpened, DisputeWon, Payment, PaymentCaptured, PaymentDeclined,
        PaymentRefunded, PaymentRequested, RefundFailed, RefundRequested, RefundSettled,
    },
    error::PaymentError,
    value_object::{DisputeStatus, PaymentStatus},
};

/// Deterministic payment id: one payment per order.
pub fn payment_id(order_id: &str) -> String {
    timada_core::id::derived(&[order_id], "payment")
}

pub struct Command<'a, E: Executor>(pub &'a E);

impl<E: Executor> Deref for Command<'_, E> {
    type Target = E;

    fn deref(&self) -> &Self::Target {
        self.0
    }
}

impl<E: Executor> Command<'_, E> {
    pub async fn load(&self, id: impl Into<String>) -> anyhow::Result<Option<PaymentState>> {
        create_projection().load(id).execute(self.0).await
    }

    async fn load_existing(&self, id: impl Into<String>) -> Result<PaymentState, PaymentError> {
        self.load(id).await?.ok_or(PaymentError::PaymentNotFound)
    }
}

/// Write-side state: just enough to guard the commands.
#[evento::projection(id = id)]
#[evento::snapshot(none)]
pub struct PaymentState {
    pub id: String,
    pub order_id: String,
    pub status: PaymentStatus,
    pub amount: Money,
    /// What the provider confirmed it gave back.
    pub refunded: Money,
    /// The reason of every refund so far — asked for, settled or failed;
    /// [`Command::refund_payment_once`] uses it as its idempotency key.
    pub refund_reasons: Vec<String>,
    /// Refunds asked for and not settled yet: their amounts are held.
    pub pending_refunds: Vec<OpenRefund>,
    /// Refunds the provider refused: nothing is held for them any more.
    pub failed_refunds: Vec<OpenRefund>,
    pub settled_refund_ids: Vec<String>,
    /// How many distinct refunds were ever asked for; the next refund's id
    /// derives from it.
    pub refund_requests: u32,
    /// Every dispute the provider reported, oldest first.
    pub disputes: Vec<DisputeState>,
}

/// A dispute as the commands need it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DisputeState {
    pub dispute_id: String,
    pub amount: Money,
    pub status: DisputeStatus,
}

/// A refund that was asked for and is not settled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenRefund {
    pub refund_id: String,
    pub amount: Money,
    pub reason: String,
}

impl PaymentState {
    /// What pending refunds hold.
    pub fn pending(&self) -> Result<Money, timada_core::MoneyError> {
        self.pending_refunds
            .iter()
            .try_fold(Money::zero(&self.amount.currency), |sum, refund| {
                sum.checked_add(&refund.amount)
            })
    }

    /// What lost disputes took back: money the shop no longer holds.
    pub fn charged_back(&self) -> Result<Money, timada_core::MoneyError> {
        self.disputes
            .iter()
            .filter(|d| d.status == DisputeStatus::Lost)
            .try_fold(Money::zero(&self.amount.currency), |sum, dispute| {
                sum.checked_add(&dispute.amount)
            })
    }

    /// Whether `amount` more can still be given back, counting what is
    /// already refunded, what lost disputes took and what pending refunds
    /// hold.
    pub(crate) fn can_refund(&self, amount: &Money) -> Result<bool, timada_core::MoneyError> {
        let total = self
            .refunded
            .checked_add(&self.charged_back()?)?
            .checked_add(&self.pending()?)?
            .checked_add(amount)?;
        Ok(total.minor <= self.amount.minor)
    }
}

// Strict and folding every event, so the version `write()` relies on is exact.
fn create_projection<E: Executor>() -> Projection<E, PaymentState> {
    Projection::new::<Payment>()
        .handler(on_payment_requested())
        .handler(on_payment_captured())
        .handler(on_payment_declined())
        .handler(on_payment_refunded())
        .handler(on_refund_requested())
        .handler(on_refund_settled())
        .handler(on_refund_failed())
        .handler(on_dispute_opened())
        .handler(on_dispute_won())
        .handler(on_dispute_lost())
        .strict()
}

#[evento::handler]
async fn on_payment_requested(
    event: Event<PaymentRequested>,
    row: &mut PaymentState,
) -> anyhow::Result<()> {
    row.id = event.aggregate_id.to_owned();
    row.order_id = event.data.order_id;
    row.refunded = Money::zero(&event.data.amount.currency);
    row.amount = event.data.amount;
    row.status = PaymentStatus::Requested;
    Ok(())
}

#[evento::handler]
async fn on_payment_captured(
    _event: Event<PaymentCaptured>,
    row: &mut PaymentState,
) -> anyhow::Result<()> {
    row.status = PaymentStatus::Captured;
    Ok(())
}

#[evento::handler]
async fn on_payment_declined(
    _event: Event<PaymentDeclined>,
    row: &mut PaymentState,
) -> anyhow::Result<()> {
    row.status = PaymentStatus::Declined;
    Ok(())
}

#[evento::handler]
async fn on_payment_refunded(
    event: Event<PaymentRefunded>,
    row: &mut PaymentState,
) -> anyhow::Result<()> {
    row.refunded = row.refunded.checked_add(&event.data.amount)?;
    if row.refunded == row.amount {
        row.status = PaymentStatus::Refunded;
    }
    // A refund that was asked for first already recorded its reason.
    if !row.refund_reasons.contains(&event.data.reason) {
        row.refund_reasons.push(event.data.reason);
    }
    Ok(())
}

#[evento::handler]
async fn on_refund_requested(
    event: Event<RefundRequested>,
    row: &mut PaymentState,
) -> anyhow::Result<()> {
    // A failed refund asked for again goes back to pending.
    if let Some(at) = row
        .failed_refunds
        .iter()
        .position(|r| r.refund_id == event.data.refund_id)
    {
        row.failed_refunds.remove(at);
    } else {
        row.refund_requests += 1;
        row.refund_reasons.push(event.data.reason.clone());
    }
    row.pending_refunds.push(OpenRefund {
        refund_id: event.data.refund_id,
        amount: event.data.amount,
        reason: event.data.reason,
    });
    Ok(())
}

#[evento::handler]
async fn on_refund_settled(
    event: Event<RefundSettled>,
    row: &mut PaymentState,
) -> anyhow::Result<()> {
    row.pending_refunds
        .retain(|r| r.refund_id != event.data.refund_id);
    row.failed_refunds
        .retain(|r| r.refund_id != event.data.refund_id);
    row.settled_refund_ids.push(event.data.refund_id);
    Ok(())
}

#[evento::handler]
async fn on_refund_failed(
    event: Event<RefundFailed>,
    row: &mut PaymentState,
) -> anyhow::Result<()> {
    if let Some(at) = row
        .pending_refunds
        .iter()
        .position(|r| r.refund_id == event.data.refund_id)
    {
        let refund = row.pending_refunds.remove(at);
        row.failed_refunds.push(refund);
    }
    Ok(())
}

#[evento::handler]
async fn on_dispute_opened(
    event: Event<DisputeOpened>,
    row: &mut PaymentState,
) -> anyhow::Result<()> {
    row.disputes.push(DisputeState {
        dispute_id: event.data.dispute_id,
        amount: event.data.amount,
        status: DisputeStatus::Open,
    });
    Ok(())
}

fn close_dispute(row: &mut PaymentState, dispute_id: &str, status: DisputeStatus) {
    if let Some(dispute) = row.disputes.iter_mut().find(|d| d.dispute_id == dispute_id) {
        dispute.status = status;
    }
}

#[evento::handler]
async fn on_dispute_won(event: Event<DisputeWon>, row: &mut PaymentState) -> anyhow::Result<()> {
    close_dispute(row, &event.data.dispute_id, DisputeStatus::Won);
    Ok(())
}

#[evento::handler]
async fn on_dispute_lost(event: Event<DisputeLost>, row: &mut PaymentState) -> anyhow::Result<()> {
    close_dispute(row, &event.data.dispute_id, DisputeStatus::Lost);
    Ok(())
}
