mod draft_invoice;
mod issue_invoice;
mod void_invoice;

pub use draft_invoice::DraftInvoice;

use evento::{Executor, Projection, metadata::Event};
use sqlx::SqlitePool;

use crate::{
    aggregator::{Invoice, InvoiceDrafted, InvoiceIssued, InvoiceVoided},
    error::InvoiceError,
    value_object::InvoiceStatus,
};

/// Deterministic invoice id: one invoice per order.
pub fn invoice_id(order_id: &str) -> String {
    timada_core::id::derived(&[order_id], "invoice")
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
}

fn create_projection<E: Executor>() -> Projection<E, InvoiceState> {
    Projection::new::<Invoice>()
        .handler(on_invoice_drafted())
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
