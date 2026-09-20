//! Everything an order or admin page needs to know about a payment, folded
//! from one `Payment` stream. Executor-backed snapshots via the bitcode derives.

use evento::{Executor, metadata::Event, projection::Projection};
use timada_core::Money;

use crate::{
    aggregator::{
        DisputeLost, DisputeOpened, DisputeWon, Payment, PaymentCaptured, PaymentDeclined,
        PaymentRefunded, PaymentRequested, RefundFailed, RefundRequested, RefundSettled,
    },
    value_object::{DisputeStatus, PaymentMethod, PaymentStatus, RefundStatus},
};

#[evento::projection(bitcode::Encode, bitcode::Decode)]
#[derive(Debug, PartialEq)]
pub struct PaymentView {
    pub id: String,
    pub order_id: String,
    pub amount: Money,
    pub method: PaymentMethod,
    pub status: PaymentStatus,
    pub psp_reference: Option<String>,
    pub declined_reason: Option<String>,
    /// What the provider confirmed it gave back.
    pub refunded: Money,
    /// Every refund that was asked for, oldest first. Refunds recorded before
    /// refunds were asked for first only show in `refunded`.
    pub refunds: Vec<RefundView>,
    /// Every dispute the provider reported, oldest first.
    pub disputes: Vec<DisputeView>,
}

/// One dispute of a payment, from the cardholder's claim to the bank's
/// decision.
#[derive(Debug, Clone, PartialEq, Eq, bitcode::Encode, bitcode::Decode)]
pub struct DisputeView {
    /// The provider's own reference.
    pub dispute_id: String,
    pub amount: Money,
    /// The provider's reason code: see [`crate::dispute_reason_label`].
    pub reason: String,
    pub status: DisputeStatus,
    pub opened_at: u64,
    /// Unix seconds: when the shop's evidence is due.
    pub respond_by: Option<u64>,
    pub closed_at: Option<u64>,
}

/// One refund of a payment, from the request to its outcome.
#[derive(Debug, Clone, PartialEq, Eq, bitcode::Encode, bitcode::Decode)]
pub struct RefundView {
    pub refund_id: String,
    pub amount: Money,
    pub reason: String,
    pub status: RefundStatus,
    pub requested_at: u64,
    pub psp_refund_reference: Option<String>,
    pub failure: Option<String>,
}

impl PaymentView {
    /// What refunds still on their way to the provider hold.
    pub fn pending_refunds(&self) -> Result<Money, timada_core::MoneyError> {
        self.refunds
            .iter()
            .filter(|r| r.status == RefundStatus::Pending)
            .try_fold(Money::zero(&self.amount.currency), |sum, refund| {
                sum.checked_add(&refund.amount)
            })
    }

    /// What can still be given back: the captured amount, less what was
    /// refunded, what lost disputes took and what pending refunds hold.
    pub fn refundable(&self) -> Result<Money, timada_core::MoneyError> {
        self.amount
            .checked_sub(&self.refunded)?
            .checked_sub(&self.charged_back()?)?
            .checked_sub(&self.pending_refunds()?)
    }

    /// The dispute the bank has not decided yet, if any. While there is one
    /// the order is not to be shipped and no refund reaches the provider.
    pub fn open_dispute(&self) -> Option<&DisputeView> {
        self.disputes
            .iter()
            .find(|d| d.status == DisputeStatus::Open)
    }

    /// What lost disputes took back.
    pub fn charged_back(&self) -> Result<Money, timada_core::MoneyError> {
        self.disputes
            .iter()
            .filter(|d| d.status == DisputeStatus::Lost)
            .try_fold(Money::zero(&self.amount.currency), |sum, dispute| {
                sum.checked_add(&dispute.amount)
            })
    }
}

pub fn create_projection<E: Executor>() -> Projection<E, PaymentView> {
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
        // `refunds`, then `disputes`, joined the snapshot.
        .revision(2)
}

pub async fn load<E: Executor>(
    executor: &E,
    id: impl Into<String>,
) -> anyhow::Result<Option<PaymentView>> {
    create_projection().load(id).execute(executor).await
}

#[evento::handler]
async fn on_payment_requested(
    event: Event<PaymentRequested>,
    row: &mut PaymentView,
) -> anyhow::Result<()> {
    row.id = event.aggregate_id.to_owned();
    row.order_id = event.data.order_id;
    row.refunded = Money::zero(&event.data.amount.currency);
    row.amount = event.data.amount;
    row.method = event.data.method;
    row.status = PaymentStatus::Requested;
    Ok(())
}

#[evento::handler]
async fn on_payment_captured(
    event: Event<PaymentCaptured>,
    row: &mut PaymentView,
) -> anyhow::Result<()> {
    row.psp_reference = Some(event.data.psp_reference);
    row.status = PaymentStatus::Captured;
    Ok(())
}

#[evento::handler]
async fn on_payment_declined(
    event: Event<PaymentDeclined>,
    row: &mut PaymentView,
) -> anyhow::Result<()> {
    row.declined_reason = Some(event.data.reason);
    row.status = PaymentStatus::Declined;
    Ok(())
}

#[evento::handler]
async fn on_payment_refunded(
    event: Event<PaymentRefunded>,
    row: &mut PaymentView,
) -> anyhow::Result<()> {
    row.refunded = row.refunded.checked_add(&event.data.amount)?;
    if row.refunded == row.amount {
        row.status = PaymentStatus::Refunded;
    }
    Ok(())
}

#[evento::handler]
async fn on_refund_requested(
    event: Event<RefundRequested>,
    row: &mut PaymentView,
) -> anyhow::Result<()> {
    // A failed refund asked for again goes back to pending.
    if let Some(refund) = row
        .refunds
        .iter_mut()
        .find(|r| r.refund_id == event.data.refund_id)
    {
        refund.status = RefundStatus::Pending;
        refund.failure = None;
        return Ok(());
    }
    let requested_at = event.timestamp;
    row.refunds.push(RefundView {
        refund_id: event.data.refund_id,
        amount: event.data.amount,
        reason: event.data.reason,
        status: RefundStatus::Pending,
        requested_at,
        psp_refund_reference: None,
        failure: None,
    });
    Ok(())
}

#[evento::handler]
async fn on_refund_settled(
    event: Event<RefundSettled>,
    row: &mut PaymentView,
) -> anyhow::Result<()> {
    if let Some(refund) = row
        .refunds
        .iter_mut()
        .find(|r| r.refund_id == event.data.refund_id)
    {
        refund.status = RefundStatus::Settled;
        refund.failure = None;
        refund.psp_refund_reference = Some(event.data.psp_refund_reference);
    }
    Ok(())
}

#[evento::handler]
async fn on_refund_failed(event: Event<RefundFailed>, row: &mut PaymentView) -> anyhow::Result<()> {
    if let Some(refund) = row
        .refunds
        .iter_mut()
        .find(|r| r.refund_id == event.data.refund_id)
    {
        refund.status = RefundStatus::Failed;
        refund.failure = Some(event.data.reason);
    }
    Ok(())
}

#[evento::handler]
async fn on_dispute_opened(
    event: Event<DisputeOpened>,
    row: &mut PaymentView,
) -> anyhow::Result<()> {
    let opened_at = event.timestamp;
    row.disputes.push(DisputeView {
        dispute_id: event.data.dispute_id,
        amount: event.data.amount,
        reason: event.data.reason,
        status: DisputeStatus::Open,
        opened_at,
        respond_by: event.data.respond_by,
        closed_at: None,
    });
    Ok(())
}

fn close_dispute(row: &mut PaymentView, dispute_id: &str, status: DisputeStatus, at: u64) {
    if let Some(dispute) = row.disputes.iter_mut().find(|d| d.dispute_id == dispute_id) {
        dispute.status = status;
        dispute.closed_at = Some(at);
    }
}

#[evento::handler]
async fn on_dispute_won(event: Event<DisputeWon>, row: &mut PaymentView) -> anyhow::Result<()> {
    close_dispute(
        row,
        &event.data.dispute_id,
        DisputeStatus::Won,
        event.timestamp,
    );
    Ok(())
}

#[evento::handler]
async fn on_dispute_lost(event: Event<DisputeLost>, row: &mut PaymentView) -> anyhow::Result<()> {
    close_dispute(
        row,
        &event.data.dispute_id,
        DisputeStatus::Lost,
        event.timestamp,
    );
    Ok(())
}
