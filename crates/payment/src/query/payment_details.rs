//! Everything an order or admin page needs to know about a payment, folded
//! from one `Payment` stream. Executor-backed snapshots via the bitcode derives.

use evento::{Executor, metadata::Event, projection::Projection};
use timada_core::Money;

use crate::{
    aggregator::{Payment, PaymentCaptured, PaymentDeclined, PaymentRefunded, PaymentRequested},
    value_object::{PaymentMethod, PaymentStatus},
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
    pub refunded: Money,
}

pub fn create_projection<E: Executor>() -> Projection<E, PaymentView> {
    Projection::new::<Payment>()
        .handler(on_payment_requested())
        .handler(on_payment_captured())
        .handler(on_payment_declined())
        .handler(on_payment_refunded())
        .strict()
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
