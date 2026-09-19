//! The invoice as a document: everything a rendering — a printable page
//! today, a PDF tomorrow — has to show, assembled once so every rendering
//! says the same thing. Nothing here is persisted.
//!
//! A French invoice must carry the seller's identity (name, address, SIREN or
//! SIRET, VAT number), the buyer, a number and a date, each line with its
//! quantity and unit price, the totals before and after VAT with the VAT per
//! rate, and — when no VAT is charged — the legal ground for it.

use sqlx::SqlitePool;
use timada_core::{Address, Money};
use timada_tax::VatLine;

use crate::{
    credit_note_list::{CreditNoteListRow, credit_notes_of_invoice},
    query::{InvoiceView, load_invoice},
    value_object::InvoiceStatus,
};

/// Who issues the invoices. A value the host provides, like a `MailerConfig`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct InvoiceIssuer {
    /// Legal name, with its form: "Timada SAS".
    pub name: String,
    /// Postal address, one entry per line.
    pub address_lines: Vec<String>,
    /// SIREN / SIRET, as it should be printed.
    pub registration: String,
    /// Intra-community VAT number.
    pub vat_number: String,
    /// Where a customer asks about an invoice (e-mail or phone).
    pub contact: String,
}

/// One printed line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentLine {
    pub label: String,
    pub quantity: u32,
    pub unit_price: Money,
    pub total: Money,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentCreditNote {
    pub number: String,
    pub issued_at: u64,
    pub reason: String,
    pub amount: Money,
}

/// A numbered invoice, ready to be rendered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvoiceDocument {
    pub issuer: InvoiceIssuer,
    pub invoice_id: String,
    pub number: String,
    /// Unix seconds: the invoice's legal date.
    pub issued_at: u64,
    pub order_id: String,
    /// The order's number when it has one, its id otherwise.
    pub order_label: String,
    pub customer_id: String,
    pub buyer: Address,
    pub lines: Vec<DocumentLine>,
    pub subtotal: Money,
    pub shipping_fee: Money,
    pub handling_fee: Money,
    /// `(label, amount)` of the promo code or voucher.
    pub discount: Option<(String, Money)>,
    /// What the customer was charged.
    pub total: Money,
    /// Whether the amounts include VAT ("TTC") or are an export's ("HT").
    pub amounts_include_vat: bool,
    /// The VAT per rate; empty for invoices older than tax zones.
    pub vat_lines: Vec<VatLine>,
    /// The legal ground when no VAT is charged.
    pub exemption_mention: Option<&'static str>,
    pub credit_notes: Vec<DocumentCreditNote>,
    /// `total` less the credit notes.
    pub net_after_credit_notes: Money,
}

impl InvoiceDocument {
    /// The pre-tax total, from the VAT summary.
    pub fn total_excl_vat(&self) -> Option<Money> {
        let first = self.vat_lines.first()?;
        let minor = self.vat_lines.iter().map(|l| l.base.minor).sum();
        Some(Money::new(minor, &first.base.currency))
    }

    pub fn vat_total(&self) -> Option<Money> {
        let first = self.vat_lines.first()?;
        let minor = self.vat_lines.iter().map(|l| l.vat.minor).sum();
        Some(Money::new(minor, &first.vat.currency))
    }
}

/// Assembles the document of an **issued** invoice; a draft has no number and
/// a voided one is not an invoice any more, so both give `None`.
pub fn invoice_document(
    issuer: &InvoiceIssuer,
    invoice: InvoiceView,
    order_number: Option<String>,
    credit_notes: Vec<CreditNoteListRow>,
) -> anyhow::Result<Option<InvoiceDocument>> {
    let (Some(number), Some(issued_at)) = (invoice.invoice_number.clone(), invoice.issued_at)
    else {
        return Ok(None);
    };
    if invoice.status != InvoiceStatus::Issued {
        return Ok(None);
    }

    let mut lines = Vec::with_capacity(invoice.lines.len());
    for line in &invoice.lines {
        lines.push(DocumentLine {
            label: line.label.clone(),
            quantity: line.quantity,
            unit_price: line.unit_price.clone(),
            total: line.total()?,
        });
    }
    let mut credited = Money::zero(&invoice.total.currency);
    let mut notes = Vec::with_capacity(credit_notes.len());
    for note in credit_notes {
        let amount = Money::new(note.amount_minor, &note.currency);
        credited = credited.checked_add(&amount)?;
        notes.push(DocumentCreditNote {
            number: note.credit_note_number,
            issued_at: note.issued_at.max(0) as u64,
            reason: note.reason,
            amount,
        });
    }
    let exemption_mention = invoice.tax.as_ref().and_then(|tax| tax.exemption_mention());

    Ok(Some(InvoiceDocument {
        issuer: issuer.clone(),
        number,
        issued_at,
        order_label: order_number.unwrap_or_else(|| invoice.order_id.clone()),
        net_after_credit_notes: invoice.total.checked_sub(&credited)?,
        amounts_include_vat: exemption_mention.is_none(),
        exemption_mention,
        vat_lines: invoice.tax.map(|tax| tax.vat_lines).unwrap_or_default(),
        discount: invoice.discount.map(|d| (d.label, d.amount)),
        invoice_id: invoice.id,
        order_id: invoice.order_id,
        customer_id: invoice.customer_id,
        buyer: invoice.billing_address,
        lines,
        subtotal: invoice.subtotal,
        shipping_fee: invoice.shipping_fee,
        handling_fee: invoice.handling_fee,
        total: invoice.total,
        credit_notes: notes,
    }))
}

/// Loads the invoice, its order's number and its credit notes, and assembles
/// the document. `None` when there is no such invoice, or it is not issued.
pub async fn load_invoice_document<E: evento::Executor>(
    executor: &E,
    db: &SqlitePool,
    issuer: &InvoiceIssuer,
    invoice_id: &str,
) -> anyhow::Result<Option<InvoiceDocument>> {
    let Some(invoice) = load_invoice(executor, invoice_id).await? else {
        return Ok(None);
    };
    let order_number =
        timada_order::order_numbers_by_ids(db, std::slice::from_ref(&invoice.order_id))
            .await?
            .remove(&invoice.order_id);
    let credit_notes = credit_notes_of_invoice(db, invoice_id).await?;
    invoice_document(issuer, invoice, order_number, credit_notes)
}
