mod approve_return;
mod cancel_return;
mod complete_return;
mod receive_return;
mod refuse_return;
mod request_return;

pub use receive_return::ReceiveReturn;
pub use request_return::{RequestReturn, RequestedLine};

use evento::{Executor, Projection, metadata::Event};
use sqlx::SqlitePool;

use crate::{
    aggregator::{
        Return, ReturnApproved, ReturnCancelled, ReturnCompleted, ReturnReceived, ReturnRefused,
        ReturnRequested,
    },
    error::ReturnError,
    value_object::{ReturnLine, ReturnPolicy, ReturnStatus},
};

/// Deterministic return id, from the RMA number the request was given.
pub fn return_id(rma_number: &str) -> String {
    timada_core::id::derived(&[rma_number], "return")
}

/// RMA numbers and the units still returnable per order line are contended
/// counters, so the commands need the pool next to the executor.
pub struct Command<'a, E: Executor> {
    pub executor: &'a E,
    pub db: SqlitePool,
    pub policy: ReturnPolicy,
}

impl<E: Executor> Command<'_, E> {
    pub async fn load(&self, id: impl Into<String>) -> anyhow::Result<Option<ReturnState>> {
        create_projection().load(id).execute(self.executor).await
    }

    async fn load_existing(&self, id: impl Into<String>) -> Result<ReturnState, ReturnError> {
        self.load(id).await?.ok_or(ReturnError::ReturnNotFound)
    }

    /// Frees the units a return was holding against its order.
    async fn release_claims(&self, return_id: &str) -> Result<(), ReturnError> {
        sqlx::query("DELETE FROM return_claim WHERE return_id = ?")
            .bind(return_id)
            .execute(&self.db)
            .await?;
        Ok(())
    }
}

/// Write-side state: just enough to guard the commands.
#[evento::projection(id = id)]
#[evento::snapshot(none)]
pub struct ReturnState {
    pub id: String,
    pub rma_number: String,
    pub order_id: String,
    pub customer_id: String,
    pub status: ReturnStatus,
    pub lines: Vec<ReturnLine>,
}

impl ReturnState {
    fn expect_status(&self, expected: ReturnStatus) -> Result<(), ReturnError> {
        if self.status == expected {
            Ok(())
        } else {
            Err(ReturnError::WrongStatus {
                expected: expected.as_str(),
                actual: self.status.as_str(),
            })
        }
    }
}

// Strict and folding every event, so the version `write()` relies on is exact.
fn create_projection<E: Executor>() -> Projection<E, ReturnState> {
    Projection::new::<Return>()
        .handler(on_return_requested())
        .handler(on_return_approved())
        .handler(on_return_refused())
        .handler(on_return_cancelled())
        .handler(on_return_received())
        .handler(on_return_completed())
        .strict()
}

#[evento::handler]
async fn on_return_requested(
    event: Event<ReturnRequested>,
    row: &mut ReturnState,
) -> anyhow::Result<()> {
    row.id = event.aggregate_id.to_owned();
    row.rma_number = event.data.rma_number;
    row.order_id = event.data.order_id;
    row.customer_id = event.data.customer_id;
    row.lines = event.data.lines;
    row.status = ReturnStatus::Requested;
    Ok(())
}

#[evento::handler]
async fn on_return_approved(
    _event: Event<ReturnApproved>,
    row: &mut ReturnState,
) -> anyhow::Result<()> {
    row.status = ReturnStatus::Approved;
    Ok(())
}

#[evento::handler]
async fn on_return_refused(
    _event: Event<ReturnRefused>,
    row: &mut ReturnState,
) -> anyhow::Result<()> {
    row.status = ReturnStatus::Refused;
    Ok(())
}

#[evento::handler]
async fn on_return_cancelled(
    _event: Event<ReturnCancelled>,
    row: &mut ReturnState,
) -> anyhow::Result<()> {
    row.status = ReturnStatus::Cancelled;
    Ok(())
}

#[evento::handler]
async fn on_return_received(
    _event: Event<ReturnReceived>,
    row: &mut ReturnState,
) -> anyhow::Result<()> {
    row.status = ReturnStatus::Received;
    Ok(())
}

#[evento::handler]
async fn on_return_completed(
    _event: Event<ReturnCompleted>,
    row: &mut ReturnState,
) -> anyhow::Result<()> {
    row.status = ReturnStatus::Completed;
    Ok(())
}
