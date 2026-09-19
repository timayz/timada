mod draft_invoice;
mod issue_credit_note;
mod issue_invoice;
mod void_invoice;

pub use draft_invoice::DraftInvoice;
pub use issue_credit_note::IssueCreditNote;

use evento::{Executor, Projection, metadata::Event};
use sqlx::SqlitePool;
use timada_core::Money;

use crate::{
    aggregator::{Invoice, InvoiceDiscountApplied, InvoiceDrafted, InvoiceIssued, InvoiceVoided},
    error::InvoiceError,
    value_object::{InvoiceStatus, invoice_total},
};

/// Deterministic invoice id: one invoice per order.
pub fn invoice_id(order_id: &str) -> String {
    timada_core::id::derived(&[order_id], "invoice")
}

/// Deterministic credit note id: one credit note per refund.
pub fn credit_note_id(refund_id: &str) -> String {
    timada_core::id::derived(&[refund_id], "credit-note")
}

/// Invoice numbers are a contended, gapless-per-order counter, so they live
/// in a write-side SQL table rather than being counted from events.
pub struct Command<'a, E: Executor> {
    pub executor: &'a E,
    pub db: SqlitePool,
}

impl<E: Executor> Command<'_, E> {
    pub async fn load(&self, id: impl Into<String>) -> anyhow::Result<Option<InvoiceState>> {
        create_projection().load(id).execute(self.executor).await
    }

    async fn load_existing(&self, id: impl Into<String>) -> Result<InvoiceState, InvoiceError> {
        self.load(id).await?.ok_or(InvoiceError::InvoiceNotFound)
    }
}

/// Write-side state: just enough to guard the commands.
#[evento::projection(id = id)]
#[evento::snapshot(none)]
pub struct InvoiceState {
    pub id: String,
    pub order_id: String,
    pub status: InvoiceStatus,
    pub invoice_number: Option<String>,
    /// Net of the discount: the most credit notes may give back.
    pub total: Money,
}

fn create_projection<E: Executor>() -> Projection<E, InvoiceState> {
    Projection::new::<Invoice>()
        .handler(on_invoice_drafted())
        .handler(on_invoice_discount_applied())
        .handler(on_invoice_issued())
        .handler(on_invoice_voided())
        .strict()
}

#[evento::handler]
async fn on_invoice_drafted(
    event: Event<InvoiceDrafted>,
    row: &mut InvoiceState,
) -> anyhow::Result<()> {
    row.id = event.aggregate_id.to_owned();
    row.order_id = event.data.order_id;
    row.status = InvoiceStatus::Draft;
    (_, row.total) = invoice_total(
        &event.data.lines,
        &event.data.shipping_fee,
        &event.data.handling_fee,
    )?;
    Ok(())
}

#[evento::handler]
async fn on_invoice_discount_applied(
    event: Event<InvoiceDiscountApplied>,
    row: &mut InvoiceState,
) -> anyhow::Result<()> {
    row.total = row.total.checked_sub(&event.data.amount)?;
    Ok(())
}

#[evento::handler]
async fn on_invoice_issued(
    event: Event<InvoiceIssued>,
    row: &mut InvoiceState,
) -> anyhow::Result<()> {
    row.status = InvoiceStatus::Issued;
    row.invoice_number = Some(event.data.invoice_number);
    Ok(())
}

#[evento::handler]
async fn on_invoice_voided(
    _event: Event<InvoiceVoided>,
    row: &mut InvoiceState,
) -> anyhow::Result<()> {
    row.status = InvoiceStatus::Voided;
    Ok(())
}
