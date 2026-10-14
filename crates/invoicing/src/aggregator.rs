//! The persisted shapes. Every variant is frozen once merged (`events.lock`):
//! a change of shape is a new variant, never a changed field.

use crate::value_object::{Address, InvoiceLine, Seller, VatSubtotal};

/// One chunk of a numbering series (`F2026`, `A2026`): its stream holds the
/// documents themselves, each numbered one more than the one before. The
/// id is [`book_id`](crate::book_id) of the series and chunk.
#[evento::aggregate(name = "timada-invoicing/Book")]
pub enum Book {
    /// An invoice, issued whole: everything it says, as it was said then.
    InvoiceIssued {
        /// Its index stream ([`invoice_id`](crate::invoice_id)).
        document_id: String,
        series: String,
        number: u64,
        /// Unix seconds.
        issued_at: u64,
        order_id: String,
        /// The short code the shopper knows the order by.
        order_reference: String,
        /// Unix seconds the order was paid at.
        paid_at: u64,
        /// [`PAYMENT_METHOD_CARD`](crate::PAYMENT_METHOD_CARD).
        payment_method: String,
        seller: Seller,
        buyer: Address,
        buyer_email: String,
        /// Where the goods go.
        delivery: Address,
        currency: String,
        lines: Vec<InvoiceLine>,
        /// By rate, highest first; empty when no VAT is charged.
        vat: Vec<VatSubtotal>,
        total_excl_vat_cents: u64,
        total_vat_cents: u64,
        total_cents: u64,
        /// [`TEMPLATE_VERSION`](crate::TEMPLATE_VERSION) when issued.
        template: u16,
        issued_by: String,
    },
    /// A credit note: money given back against an invoice.
    CreditNoteIssued {
        /// Its index stream ([`credit_note_id`](crate::credit_note_id)).
        document_id: String,
        series: String,
        number: u64,
        issued_at: u64,
        /// The invoice it corrects, by index stream and by number.
        invoice_document_id: String,
        invoice_label: String,
        order_id: String,
        order_reference: String,
        /// The provider's id of the refund.
        refund_id: String,
        seller: Seller,
        buyer: Address,
        buyer_email: String,
        currency: String,
        /// By rate, highest first; empty when no VAT is charged.
        vat: Vec<VatSubtotal>,
        total_excl_vat_cents: u64,
        total_vat_cents: u64,
        total_cents: u64,
        template: u16,
        issued_by: String,
    },
}

/// The index `document → book`: one stream per document, with the
/// deterministic id [`invoice_id`](crate::invoice_id) or
/// [`credit_note_id`](crate::credit_note_id).
#[evento::aggregate(name = "timada-invoicing/Document")]
pub enum Document {
    /// Taken first, before a number: the series the document goes in, so
    /// that a retry looks there, and the one writer that goes ahead.
    Reserved { series: String },
    Recorded {
        book_id: String,
        /// `F2026-000042`.
        label: String,
    },
}
