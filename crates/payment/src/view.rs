//! The write-side view of a payment, replayed from its events.
//!
//! Unlike [`crate::projection`] — the eventual-consistent SQL table that backs
//! the admin page — this is loaded on demand straight from the event store, so
//! a reader always sees every event committed so far. The fulfillment saga uses
//! it to map a payment aggregate id (all a `ChargeCaptured` event carries) back
//! to the order it pays for.

use evento::ProjectionAggregate;
use evento::metadata::Event;
use evento::projection::Projection;
use timada_core::{Executor, Money};

use crate::aggregate::{ChargeCaptured, ChargeFailed, ChargeRefunded, ChargeRequested, Payment};

/// Where a payment stands. `Requested` means the provider has not answered yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, bitcode::Encode, bitcode::Decode)]
pub enum PaymentStatus {
    #[default]
    Requested,
    Captured,
    Failed,
    Refunded,
}

#[evento::projection(bitcode::Encode, bitcode::Decode)]
#[derive(Debug, PartialEq, Eq)]
pub struct PaymentView {
    pub id: String,
    pub order_id: String,
    pub amount: Money,
    pub provider: String,
    /// The gateway's own charge reference — `Some` once captured, and kept
    /// after a refund so the charge stays traceable.
    pub provider_charge_ref: Option<String>,
    /// Why the charge was refused — `Some` only once failed.
    pub reason: Option<String>,
    pub status: PaymentStatus,
}

impl ProjectionAggregate for PaymentView {
    fn aggregate_id(&self) -> String {
        self.id.to_owned()
    }
}

#[evento::handler]
async fn apply_requested(
    event: Event<ChargeRequested>,
    view: &mut PaymentView,
) -> anyhow::Result<()> {
    view.id = event.aggregate_id.clone();
    view.order_id = event.data.order_id.clone();
    view.amount = event.data.amount;
    view.provider = event.data.provider.clone();
    view.status = PaymentStatus::Requested;
    Ok(())
}

#[evento::handler]
async fn apply_captured(
    event: Event<ChargeCaptured>,
    view: &mut PaymentView,
) -> anyhow::Result<()> {
    view.provider_charge_ref = Some(event.data.provider_charge_ref.clone());
    view.reason = None;
    view.status = PaymentStatus::Captured;
    Ok(())
}

#[evento::handler]
async fn apply_failed(event: Event<ChargeFailed>, view: &mut PaymentView) -> anyhow::Result<()> {
    view.reason = Some(event.data.reason.clone());
    view.status = PaymentStatus::Failed;
    Ok(())
}

#[evento::handler]
async fn apply_refunded(
    _event: Event<ChargeRefunded>,
    view: &mut PaymentView,
) -> anyhow::Result<()> {
    view.status = PaymentStatus::Refunded;
    Ok(())
}

/// Replay one payment. `None` means no such aggregate.
pub async fn load_payment(
    executor: &Executor,
    payment_id: &str,
) -> anyhow::Result<Option<PaymentView>> {
    Projection::<_, PaymentView>::new::<Payment>()
        .handler(apply_requested())
        .handler(apply_captured())
        .handler(apply_failed())
        .handler(apply_refunded())
        .strict()
        .load(payment_id)
        .execute(executor)
        .await
}
