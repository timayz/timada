//! The invoice, replayed from its events.
//!
//! This is what the printable document renders and what
//! [`issue_credit_note`](crate::commands::issue_credit_note) decides against —
//! never the eventually-consistent `admin_invoice_list` table, which exists
//! only to list invoices cheaply.

use evento::ProjectionAggregate;
use evento::metadata::Event;
use evento::projection::Projection;
use timada_core::{Executor, Money};

use crate::aggregate::{CreditNoteIssued, Invoice, InvoiceIssued, InvoiceLine, Party};

/// One invoice, rebuilt from its event stream.
#[evento::projection(bitcode::Encode, bitcode::Decode)]
#[derive(Debug, PartialEq, Eq)]
pub struct InvoiceView {
    pub id: String,
    pub order_id: String,
    pub invoice_number: String,
    pub seller: Party,
    pub buyer: Party,
    pub lines: Vec<InvoiceLine>,
    /// The code redeemed at checkout, if any, and what it took off.
    pub discount_code: Option<String>,
    pub discount_amount: Option<Money>,
    pub total_net: Money,
    pub total_tax: Money,
    /// Tax-inclusive: `total_net + total_tax`, and the amount that was charged.
    pub total_gross: Money,
    /// `Some` once the invoice has been reversed.
    pub credit_note_number: Option<String>,
    pub credit_note_reason: Option<String>,
    /// When the invoice was issued (the `InvoiceIssued` event's timestamp).
    pub issued_at_ms: i64,
}

impl ProjectionAggregate for InvoiceView {
    fn aggregate_id(&self) -> String {
        self.id.to_owned()
    }
}

impl InvoiceView {
    /// Has this invoice been reversed by a credit note?
    pub fn is_credited(&self) -> bool {
        self.credit_note_number.is_some()
    }

    /// Issue date as `YYYY-MM-DD` (UTC), for the printed document.
    pub fn issued_on(&self) -> String {
        timada_core::format_utc_date(self.issued_at_ms)
    }
}

#[evento::handler]
async fn apply_issued(event: Event<InvoiceIssued>, view: &mut InvoiceView) -> anyhow::Result<()> {
    view.id = event.aggregate_id.clone();
    view.order_id = event.data.order_id.clone();
    view.invoice_number = event.data.invoice_number.clone();
    view.seller = event.data.seller.clone();
    view.buyer = event.data.buyer.clone();
    view.lines = event.data.lines.clone();
    view.discount_code = event.data.discount_code.clone();
    view.discount_amount = event.data.discount_amount;
    view.total_net = event.data.total_net;
    view.total_tax = event.data.total_tax;
    view.total_gross = event.data.total_gross;
    view.issued_at_ms = i64::try_from(event.timestamp)
        .unwrap_or_default()
        .saturating_mul(1000)
        .saturating_add(i64::from(event.timestamp_subsec));
    Ok(())
}

/// The invoice's own figures are left untouched — a credit note reverses the
/// document, it does not rewrite it.
#[evento::handler]
async fn apply_credit_note(
    event: Event<CreditNoteIssued>,
    view: &mut InvoiceView,
) -> anyhow::Result<()> {
    view.credit_note_number = Some(event.data.credit_note_number.clone());
    view.credit_note_reason = Some(event.data.reason.clone());
    Ok(())
}

/// Replay one invoice. `None` means no invoice was ever issued for it.
pub async fn load_invoice(
    executor: &Executor,
    invoice_id: &str,
) -> anyhow::Result<Option<InvoiceView>> {
    Projection::<_, InvoiceView>::new::<Invoice>()
        .handler(apply_issued())
        .handler(apply_credit_note())
        .strict()
        .load(invoice_id)
        .execute(executor)
        .await
}
