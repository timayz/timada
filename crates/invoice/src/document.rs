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
    query::{CreditNoteView, InvoiceView, load_credit_note, load_invoice},
    value_object::InvoiceStatus,
    vat_journal::apportion_credit,
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
    /// Worded for the customer: see [`credit_reason_label`].
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
    /// The business the invoice is for: its name and VAT number, to print
    /// with the address.
    pub company: Option<timada_tax::BusinessBuyer>,
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
    /// What to print about the VAT regime: the exemption, or the destination
    /// VAT of a distance sale inside the EU.
    pub regime_mention: Option<&'static str>,
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
            reason: credit_reason_label(&note.reason),
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
        regime_mention: invoice.tax.as_ref().and_then(|tax| {
            timada_tax::regime_mention(tax.treatment, invoice.reverse_charge.is_some())
        }),
        company: invoice.company,
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

impl InvoiceDocument {
    /// What goes above the address when the invoice is a business's: its
    /// name, then its VAT number — which such an invoice must show.
    pub fn company_lines(&self) -> Vec<String> {
        company_lines(self.company.as_ref())
    }
}

fn company_lines(company: Option<&timada_tax::BusinessBuyer>) -> Vec<String> {
    company
        .into_iter()
        .flat_map(|company| {
            [
                company.company_name.clone(),
                format!("N° TVA : {}", company.vat_number),
            ]
        })
        .collect()
}

/// A credit note ("avoir"), ready to be rendered. Like the invoice it
/// corrects, it names both parties and says how much VAT goes back — spread
/// over the invoice's rates the way the VAT journal spreads it, so the
/// document and the return agree to the cent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreditNoteDocument {
    pub issuer: InvoiceIssuer,
    pub credit_note_id: String,
    pub number: String,
    /// Unix seconds: the credit note's legal date.
    pub issued_at: u64,
    pub invoice_id: String,
    /// The invoice it corrects, which it must name.
    pub invoice_number: String,
    pub invoice_issued_at: Option<u64>,
    pub order_id: String,
    /// The order's number when it has one, its id otherwise.
    pub order_label: String,
    pub customer_id: String,
    pub buyer: Address,
    pub company: Option<timada_tax::BusinessBuyer>,
    /// Why, worded for the customer.
    pub reason: String,
    /// What is credited, as the invoice counts: VAT included, or an export's
    /// pre-tax amount.
    pub amount: Money,
    pub amounts_include_vat: bool,
    /// The credited amount per VAT rate; empty when the invoice is older
    /// than tax zones.
    pub vat_lines: Vec<VatLine>,
    pub regime_mention: Option<&'static str>,
}

impl CreditNoteDocument {
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

    pub fn company_lines(&self) -> Vec<String> {
        company_lines(self.company.as_ref())
    }
}

/// What a credit note issued for a lost dispute is keyed by and gives as its
/// reason: there is no refund of the shop's behind it — the bank took the
/// money back — so the operator decides whether the sale is cancelled. One
/// credit note per dispute, however often it is asked for.
pub fn dispute_credit_reference(dispute_id: &str) -> String {
    format!("dispute {dispute_id}")
}

/// A refund's reason as a document words it. Refunds are asked for by code
/// ("return R2026-000003", "order cancelled: …") or by an operator, whose
/// words are kept.
pub fn credit_reason_label(reason: &str) -> String {
    if let Some(rma) = reason.strip_prefix("return ") {
        return format!("Retour {rma}");
    }
    if let Some(dispute) = reason.strip_prefix("dispute ") {
        return format!("Litige bancaire {dispute}");
    }
    match reason.strip_prefix("order cancelled") {
        Some("") => "Commande annulée".to_owned(),
        Some(why) => match why.strip_prefix(": ") {
            Some(why) => format!(
                "Commande annulée : {}",
                timada_order::cancellation_reason_label(why)
            ),
            None => reason.to_owned(),
        },
        None => reason.to_owned(),
    }
}

/// Assembles the document of a credit note from the note and the invoice it
/// corrects.
pub fn credit_note_document(
    issuer: &InvoiceIssuer,
    note: CreditNoteView,
    invoice: InvoiceView,
    order_number: Option<String>,
) -> CreditNoteDocument {
    let currency = note.amount.currency.clone();
    let vat_lines = invoice
        .tax
        .as_ref()
        .map(|tax| apportion_credit(&tax.vat_lines, note.amount.minor))
        .unwrap_or_default()
        .into_iter()
        .map(|share| VatLine {
            rate_bp: share.rate_bp,
            base: Money::new(share.base_minor, &currency),
            vat: Money::new(share.vat_minor, &currency),
            total: Money::new(share.base_minor + share.vat_minor, &currency),
        })
        .collect();
    let exempt = invoice
        .tax
        .as_ref()
        .is_some_and(|tax| tax.exemption_mention().is_some());

    CreditNoteDocument {
        issuer: issuer.clone(),
        number: note.credit_note_number,
        issued_at: note.issued_at,
        invoice_number: note.invoice_number,
        invoice_issued_at: invoice.issued_at,
        order_label: order_number.unwrap_or_else(|| note.order_id.clone()),
        reason: credit_reason_label(&note.reason),
        amounts_include_vat: !exempt,
        regime_mention: invoice.regime_mention(),
        vat_lines,
        credit_note_id: note.id,
        invoice_id: note.invoice_id,
        order_id: note.order_id,
        amount: note.amount,
        customer_id: invoice.customer_id,
        buyer: invoice.billing_address,
        company: invoice.company,
    }
}

/// Loads a credit note, its invoice and the order's number — all from the
/// event store, no read model to wait for — and assembles the document.
/// `None` when there is no such credit note.
pub async fn load_credit_note_document<E: evento::Executor>(
    executor: &E,
    issuer: &InvoiceIssuer,
    credit_note_id: &str,
) -> anyhow::Result<Option<CreditNoteDocument>> {
    let Some(note) = load_credit_note(executor, credit_note_id).await? else {
        return Ok(None);
    };
    let Some(invoice) = load_invoice(executor, &note.invoice_id).await? else {
        anyhow::bail!(
            "credit note {} corrects invoice {}, which cannot be loaded",
            note.id,
            note.invoice_id
        );
    };
    let order_number = timada_order::load_order_details(executor, &note.order_id)
        .await?
        .and_then(|order| order.order_number);
    Ok(Some(credit_note_document(
        issuer,
        note,
        invoice,
        order_number,
    )))
}

#[cfg(test)]
mod tests {
    use super::credit_reason_label;

    #[test]
    fn a_refund_reason_is_worded_for_the_customer() {
        assert_eq!(
            credit_reason_label("return R2026-000003"),
            "Retour R2026-000003"
        );
        assert_eq!(credit_reason_label("order cancelled"), "Commande annulée");
        assert_eq!(
            credit_reason_label("order cancelled: payment declined"),
            "Commande annulée : paiement refusé"
        );
        assert_eq!(
            credit_reason_label("order cancelled: changement d'avis"),
            "Commande annulée : changement d'avis"
        );
        assert_eq!(
            credit_reason_label(&super::dispute_credit_reference("dp_1")),
            "Litige bancaire dp_1"
        );
        // An operator's words are kept.
        assert_eq!(credit_reason_label("geste commercial"), "geste commercial");
        assert_eq!(credit_reason_label("order cancelledX"), "order cancelledX");
    }
}
