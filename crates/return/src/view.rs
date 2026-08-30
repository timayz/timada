//! The write-side view of a return, replayed from its events.

use evento::ProjectionAggregate;
use evento::metadata::Event;
use evento::projection::Projection;
use timada_core::Executor;

use crate::aggregate::{Return, ReturnApproved, ReturnRefunded, ReturnRejected, ReturnRequested};

/// Where a return stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, bitcode::Encode, bitcode::Decode)]
pub enum ReturnStatus {
    #[default]
    Requested,
    Approved,
    Rejected,
    /// Terminal.
    Refunded,
}

impl ReturnStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Requested => "requested",
            Self::Approved => "approved",
            Self::Rejected => "rejected",
            Self::Refunded => "refunded",
        }
    }
}

#[evento::projection(bitcode::Encode, bitcode::Decode)]
#[derive(Debug, PartialEq, Eq)]
pub struct ReturnView {
    pub id: String,
    pub order_id: String,
    /// The customer's latest stated reason.
    pub reason: String,
    pub status: ReturnStatus,
    /// The admin's reason, kept after a re-request so the history reads.
    pub reject_reason: Option<String>,
    /// When the latest request was made, epoch milliseconds.
    pub requested_at: i64,
}

impl ProjectionAggregate for ReturnView {
    fn aggregate_id(&self) -> String {
        self.id.to_owned()
    }
}

#[evento::handler]
async fn apply_requested(
    event: Event<ReturnRequested>,
    view: &mut ReturnView,
) -> anyhow::Result<()> {
    view.id = event.aggregate_id.clone();
    view.order_id = event.data.order_id.clone();
    view.reason = event.data.reason.clone();
    view.status = ReturnStatus::Requested;
    view.requested_at = i64::try_from(event.timestamp)
        .unwrap_or_default()
        .saturating_mul(1000)
        .saturating_add(i64::from(event.timestamp_subsec));
    Ok(())
}

#[evento::handler]
async fn apply_approved(
    _event: Event<ReturnApproved>,
    view: &mut ReturnView,
) -> anyhow::Result<()> {
    view.status = ReturnStatus::Approved;
    Ok(())
}

#[evento::handler]
async fn apply_rejected(event: Event<ReturnRejected>, view: &mut ReturnView) -> anyhow::Result<()> {
    view.reject_reason = Some(event.data.reason.clone());
    view.status = ReturnStatus::Rejected;
    Ok(())
}

#[evento::handler]
async fn apply_refunded(
    _event: Event<ReturnRefunded>,
    view: &mut ReturnView,
) -> anyhow::Result<()> {
    view.status = ReturnStatus::Refunded;
    Ok(())
}

/// Replay one return. `None` means the order has never had one.
pub async fn load_return(
    executor: &Executor,
    return_id: &str,
) -> anyhow::Result<Option<ReturnView>> {
    Projection::<_, ReturnView>::new::<Return>()
        .handler(apply_requested())
        .handler(apply_approved())
        .handler(apply_rejected())
        .handler(apply_refunded())
        .strict()
        .load(return_id)
        .execute(executor)
        .await
}
