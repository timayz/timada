//! One credit note ("avoir"), folded from its single-event stream.
//! Executor-backed snapshots via the bitcode derives.

use evento::{Executor, metadata::Event, projection::Projection};
use timada_core::Money;

use crate::aggregator::{CreditNote, CreditNoteIssued};

#[evento::projection(bitcode::Encode, bitcode::Decode)]
#[derive(Debug, PartialEq)]
pub struct CreditNoteView {
    pub id: String,
    pub credit_note_number: String,
    pub refund_id: String,
    pub invoice_id: String,
    pub invoice_number: String,
    pub order_id: String,
    pub amount: Money,
    pub reason: String,
    /// Unix seconds of `CreditNoteIssued`.
    pub issued_at: u64,
}

pub fn create_projection<E: Executor>() -> Projection<E, CreditNoteView> {
    Projection::new::<CreditNote>()
        .handler(on_credit_note_issued())
        .strict()
}

pub async fn load<E: Executor>(
    executor: &E,
    id: impl Into<String>,
) -> anyhow::Result<Option<CreditNoteView>> {
    create_projection().load(id).execute(executor).await
}

#[evento::handler]
async fn on_credit_note_issued(
    event: Event<CreditNoteIssued>,
    row: &mut CreditNoteView,
) -> anyhow::Result<()> {
    row.id = event.aggregate_id.to_owned();
    row.issued_at = event.timestamp;
    row.credit_note_number = event.data.credit_note_number;
    row.refund_id = event.data.refund_id;
    row.invoice_id = event.data.invoice_id;
    row.invoice_number = event.data.invoice_number;
    row.order_id = event.data.order_id;
    row.amount = event.data.amount;
    row.reason = event.data.reason;
    Ok(())
}
