mod capture_payment;
mod decline_payment;
mod refund_payment;
mod request_payment;

use std::ops::Deref;

pub use request_payment::RequestPayment;

use evento::{Executor, Projection, metadata::Event};
use timada_core::Money;

use crate::{
    aggregator::{Payment, PaymentCaptured, PaymentDeclined, PaymentRefunded, PaymentRequested},
    error::PaymentError,
    value_object::PaymentStatus,
};

/// Deterministic payment id: one payment per order.
pub fn payment_id(order_id: &str) -> String {
    timada_core::id::derived(&[order_id], "payment")
}

pub struct Command<E: Executor>(pub E);

impl<E: Executor> Deref for Command<E> {
    type Target = E;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<E: Executor> Command<E> {
    pub async fn load(&self, id: impl Into<String>) -> anyhow::Result<Option<PaymentState>> {
        create_projection().load(id).execute(&self.0).await
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
    pub refunded: Money,
}

// Strict and folding every event, so the version `write()` relies on is exact.
fn create_projection<E: Executor>() -> Projection<E, PaymentState> {
    Projection::new::<Payment>()
        .handler(on_payment_requested())
        .handler(on_payment_captured())
        .handler(on_payment_declined())
        .handler(on_payment_refunded())
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
    Ok(())
}
