//! Everything an order or admin page needs to know about a payment, folded
//! from one `Payment` stream. Executor-backed snapshots via the bitcode derives.

use evento::{Executor, metadata::Event, projection::Projection};
use timada_core::Money;

use crate::{
    aggregator::{
        Payment, PaymentCaptured, PaymentDeclined, PaymentRefunded, PaymentRequested, RefundFailed,
        RefundRequested, RefundSettled,
    },
    value_object::{PaymentMethod, PaymentStatus, RefundStatus},
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
    /// refunded and what pending refunds hold.
    pub fn refundable(&self) -> Result<Money, timada_core::MoneyError> {
        self.amount
            .checked_sub(&self.refunded)?
            .checked_sub(&self.pending_refunds()?)
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
        .strict()
        // `refunds` joined the snapshot.
        .revision(1)
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
