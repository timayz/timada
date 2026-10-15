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
        /// The company that issued it (the seller), whichever tenant sold.
        company_id: String,
        /// How it is taxed ([`Regime`](crate::tax::Regime) as text).
        regime: String,
        /// The country whose VAT applies, or where the goods went.
        tax_country: String,
        /// The buyer's EU VAT number, when it bought as a business.
        buyer_vat_number: Option<String>,
        /// VIES's reference for the check of that number.
        vies_reference: Option<String>,
        /// The year's EU distance sales excluding VAT, this one included
        /// (only the invoices of a series count, in their order).
        eu_distance_sales_cents: u64,
        /// Whether destination VAT applies for the rest of the year.
        eu_threshold_passed: bool,
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
        /// Copied from the invoice it corrects.
        company_id: String,
        regime: String,
        tax_country: String,
        buyer_vat_number: Option<String>,
    },
}

/// The index `document → book`: one stream per document, with the
/// deterministic id [`invoice_id`](crate::invoice_id) or
/// [`credit_note_id`](crate::credit_note_id).
#[evento::aggregate(name = "timada-invoicing/Document")]
pub enum Document {
    /// Taken first, before a number: the series the document goes in, so
    /// that a retry looks there, and the one writer that goes ahead.
    Reserved {
        series: String,
        /// The calendar year of the series.
        year: u16,
    },
    Recorded {
        book_id: String,
        /// `F2026-000042`.
        label: String,
    },
}

/// A company's EU distance sales of one year (sales to consumers in other EU
/// countries), one stream per company, year and chunk, id =
/// [`ledger_id`](crate::ledger_id). It lives in the platform store, next to
/// the company, so every tenant of the company counts against one total.
#[evento::aggregate(name = "timada-invoicing/DistanceLedger")]
pub enum DistanceLedger {
    /// One sale counted, with the decision it got, so a retry gets the same.
    Counted {
        /// The invoice's index stream (in its tenant's store).
        document_id: String,
        tenant_id: String,
        /// [`Regime`](crate::tax::Regime) as text.
        regime: String,
        tax_country: String,
        /// The rate of each line, in basis points.
        rates: Vec<Option<u32>>,
        /// The year's total excluding VAT, this sale included.
        sales_cents: u64,
        passed: bool,
    },
}
