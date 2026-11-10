//! The persisted shapes. Every variant is frozen once merged (`events.lock`):
//! a change of shape is a new variant, never a changed field.

pub mod book {
    use crate::rates::ExchangeRate;
    use crate::value_object::{Address, InvoiceLine, Seller, VatSubtotal};

    /// One chunk of a numbering series (`F2026`, `A2026`): its stream holds the
    /// documents themselves, each numbered one more than the one before. The
    /// id is [`book_id`](crate::book_id) of the series and chunk; a chunk holds
    /// at most [`BOOK_CHUNK`](crate::BOOK_CHUNK) events, and [`BookPointer`]
    /// names the open one.
    #[evento::aggregate(name = "timada-invoicing/Book")]
    pub enum Book {
        /// The first event of every chunk after the first: the number the full
        /// chunk before it ended with, so a chunk numbers on its own.
        Opened { last_number: u64 },
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
        /// The discount code an invoice's order was paid with, written with its
        /// `InvoiceIssued`: its lines are at full price, `amount_cents` (VAT
        /// included) came off them, and its VAT and totals are of what was paid.
        InvoiceDiscounted {
            document_id: String,
            code: String,
            amount_cents: u64,
        },
        /// The shipping an invoice's order was charged, written with its
        /// `InvoiceIssued`: `amount_cents` (VAT included) came on top of the
        /// goods, taxed at their rates in proportion; its VAT and totals
        /// include it.
        InvoiceShippingCharged {
            document_id: String,
            name: String,
            amount_cents: u64,
        },
        /// The euro rate of a document priced in another currency, written with
        /// its `InvoiceIssued` or `CreditNoteIssued`: the rate of the day of
        /// issue (for a credit note, of the invoice it corrects), which the
        /// sale was counted at against the threshold of EU distance sales, and
        /// the document's VAT at that rate, in euro cents, which the document
        /// also states (CGI art. 289; directive 2006/112/CE art. 230).
        EuroRateRecorded {
            document_id: String,
            rate: ExchangeRate,
            total_vat_eur_cents: u64,
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

    /// Which chunk of a series is open: one stream per series, id
    /// [`book_pointer_id`](crate::book_pointer_id), rolled once per
    /// [`BOOK_CHUNK`](crate::BOOK_CHUNK) events of the series. Without one the
    /// series is in chunk 0. The writer that finds a chunk full opens the next
    /// with [`Book::Opened`], then rolls the pointer.
    #[evento::aggregate(name = "timada-invoicing/BookPointer")]
    pub enum BookPointer {
        Rolled { chunk: u32 },
    }
}

pub mod document {
    use crate::value_object::IssuedDocument;

    /// The index `document → book`, and the document itself once issued: one
    /// stream per document, with the deterministic id [`invoice_id`](crate::invoice_id) or
    /// [`credit_note_id`](crate::credit_note_id).
    #[evento::aggregate(name = "timada-invoicing/Document")]
    pub enum Document {
        /// Taken first, before a number: the series the document goes in, so
        /// that a retry looks there, and the one writer that goes ahead.
        Reserved {
            series: String,
            /// The calendar year of the series.
            year: u16,
            /// The chunk of the series open when reserved: the document is
            /// written to that chunk or a later one, so a retry looks from
            /// there.
            chunk: u32,
            /// For an invoice, the chunk of the company's distance ledger of
            /// the year open when reserved, where a retry starts looking for
            /// its count; a credit note is never counted.
            ledger_chunk: Option<u32>,
        },
        /// The document has its number: the book that holds it, and the
        /// document itself as the book says it, copied so that a page reads it
        /// from this one small stream. The book stays the numbering authority.
        Recorded {
            book_id: String,
            document: IssuedDocument,
        },
    }
}

pub mod ledger {
    use crate::rates::ExchangeRate;

    /// A company's EU distance sales of one year (sales to consumers in other EU
    /// countries), one stream per company, year and chunk, id =
    /// [`ledger_id`](crate::ledger_id); [`LedgerPointer`] names the open chunk.
    /// It lives in the default tenant of the store, next to the company, so
    /// every tenant of the company counts against one total.
    #[evento::aggregate(name = "timada-invoicing/DistanceLedger")]
    pub enum DistanceLedger {
        /// The first event of every chunk after the first: the total the full
        /// chunk before it ended with.
        Opened { sales_cents: u64, passed: bool },
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
        /// The euro rate a sale in another currency was counted at, written
        /// with its `Counted`, so a retry records the same rate on the invoice.
        Converted {
            document_id: String,
            tenant_id: String,
            rate: ExchangeRate,
        },
    }

    /// Which chunk of a company's distance ledger of a year is open: one stream
    /// per company and year, id [`ledger_pointer_id`](crate::ledger_pointer_id),
    /// rolled once per [`BOOK_CHUNK`](crate::BOOK_CHUNK) events. Without one the
    /// ledger is in chunk 0.
    #[evento::aggregate(name = "timada-invoicing/LedgerPointer")]
    pub enum LedgerPointer {
        Rolled { chunk: u32 },
    }
}
