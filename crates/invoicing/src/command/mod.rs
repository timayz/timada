//! Write side: the command gateway, the strict write models it loads, and
//! the issuing of documents.
//!
//! Issuing a document goes in three steps, each safe to repeat:
//!
//! 1. *Reserve*: the document's index stream records the series it goes in
//!    and the chunk of the series open at the time (version 0, so one
//!    writer wins). A retry, even in another year, keeps looking in that
//!    series, from that chunk.
//! 2. *Issue*: the document is appended to the open chunk of the series
//!    with the version the chunk was read at, its number one more than the
//!    last. Two writers racing for a number: one append fails and reads
//!    again. Before taking a number, the chunks from the reserved one to
//!    the open one are searched for the document, so a retry after a crash
//!    between steps 2 and 3 never takes a second one.
//! 3. *Record*: the index points at the chunk and carries a copy of the
//!    document, so a page reads the document from its own small stream and
//!    never replays a book.
//!
//! # Chunks
//!
//! A series (a *book*) is written in chunks of at most [`BOOK_CHUNK`]
//! events, each a stream of its own, and the series' pointer stream names
//! the open one: issuing reads the pointer and that one chunk, however many
//! documents the series holds. The writer that finds the open chunk full
//! opens the next with `Opened`, carrying the last number, then rolls the
//! pointer, and reads again. Nothing is ever appended to a full chunk
//! (every writer checks the version it read against the cap before writing
//! with it), so every writer opening the next one carries the same number,
//! and either write losing its race is the work done. Loading a chunk
//! reads its last number and the ids of its documents, never the documents
//! themselves: a document is read from its chunk once, when a retry finds
//! it there and records it. The distance ledger ([`ledger`]) is chunked the
//! same way.

mod ledger;

pub use ledger::*;

use std::collections::{BTreeMap, BTreeSet};
use std::ops::Deref;

use evento::{Executor, Projection, ProjectionAggregate, WriteError, metadata::Event};

use crate::aggregator::book::{
    Book, BookPointer, CreditNoteIssued, EuroRateRecorded, InvoiceDiscounted, InvoiceIssued,
    InvoiceShippingCharged, Opened, Rolled,
};
use crate::aggregator::document::{Document, Recorded, Reserved};
use crate::error::InvoicingError;
use crate::rates::{EURO, ExchangeRate};
use crate::tax::{
    Destination, DistanceSales, Regime, TaxProfile, decide, is_distance_sale, needs_euro_rate,
};
use crate::value_object::{
    Address, BOOK_CHUNK, CREDIT_NOTE_PREFIX, DocumentKind, INVOICE_PREFIX, InvoiceDiscount,
    InvoiceLine, InvoiceShipping, IssuedDocument, Seller, TEMPLATE_VERSION, book_id,
    book_pointer_id, credit_note_id, document_label, invoice_id, prorate, series_code, series_of,
    spread, vat_breakdown, year_of,
};

/// How often a write lost to another writer is tried again.
const ATTEMPTS: usize = 10;

/// How many events a chunk holds: [`BOOK_CHUNK`], or three in the unit
/// tests so they can fill chunks.
const fn chunk_size() -> u16 {
    if cfg!(test) { 3 } else { BOOK_CHUNK }
}

/// The command gateway over a tenant's streams.
pub struct Command<E: Executor> {
    executor: E,
    tenant: String,
}

impl<E: Executor> Command<E> {
    /// The gateway of one tenant: every stream it writes, loads or reads is
    /// that tenant's. `tenant` must not be empty (the default tenant holds
    /// the platform's own streams).
    pub fn new(executor: E, tenant: impl Into<String>) -> Self {
        let tenant = tenant.into();
        debug_assert!(!tenant.is_empty(), "a tenant gateway needs a tenant");
        Self { executor, tenant }
    }

    /// A gateway naming no tenant: for a store that is one tenant's already
    /// (a storefront's connection, which the store API serves in its key's
    /// tenant whatever its requests name). Every call then acts in the
    /// default tenant, which that store maps to its own.
    pub fn unscoped(executor: E) -> Self {
        Self {
            executor,
            tenant: String::new(),
        }
    }

    /// The tenant every call acts in.
    pub fn tenant(&self) -> &str {
        &self.tenant
    }

    /// The store the tenant's streams live in.
    pub fn executor(&self) -> &E {
        &self.executor
    }
}

impl<E: Executor> Deref for Command<E> {
    type Target = E;

    fn deref(&self) -> &Self::Target {
        &self.executor
    }
}

// =============================================================================
// Book
// =============================================================================

/// A chunk of a series as issuing reads it: the last number and which
/// documents it holds, never the documents themselves (see
/// [`BookDocuments`]).
#[evento::projection(id = id, name = "timada-invoicing/BookState")]
#[evento::snapshot(none)]
#[derive(Debug, PartialEq, Eq)]
pub struct BookState {
    pub id: String,
    /// The number of the last document; the one the chunk before ended
    /// with for a chunk with none yet, `0` in the first chunk.
    pub last_number: u64,
    /// The index streams of its documents.
    pub documents: BTreeSet<String>,
}

pub fn book_projection<E: Executor>() -> Projection<E, BookState> {
    Projection::new::<Book>()
        .handler(on_opened())
        .handler(on_invoice_issued())
        .handler(on_credit_note_issued())
        .skip::<InvoiceDiscounted>()
        .skip::<InvoiceShippingCharged>()
        .skip::<EuroRateRecorded>()
        .strict()
}

#[evento::handler]
async fn on_opened(event: Event<Opened>, book: &mut BookState) -> anyhow::Result<()> {
    book.id = event.aggregate_id.to_owned();
    book.last_number = event.data.last_number;
    Ok(())
}

#[evento::handler]
async fn on_invoice_issued(
    event: Event<InvoiceIssued>,
    book: &mut BookState,
) -> anyhow::Result<()> {
    book.id = event.aggregate_id.to_owned();
    book.last_number = event.data.number;
    book.documents.insert(event.data.document_id.to_owned());
    Ok(())
}

#[evento::handler]
async fn on_credit_note_issued(
    event: Event<CreditNoteIssued>,
    book: &mut BookState,
) -> anyhow::Result<()> {
    book.id = event.aggregate_id.to_owned();
    book.last_number = event.data.number;
    book.documents.insert(event.data.document_id.to_owned());
    Ok(())
}

/// The documents of a chunk, whole: read once, when a retry finds its
/// document in a chunk and records it.
#[evento::projection(id = id, name = "timada-invoicing/BookDocuments")]
#[evento::snapshot(none)]
#[derive(Debug, PartialEq, Eq)]
pub struct BookDocuments {
    pub id: String,
    /// By index stream.
    pub documents: BTreeMap<String, IssuedDocument>,
}

pub fn book_documents_projection<E: Executor>() -> Projection<E, BookDocuments> {
    Projection::new::<Book>()
        .skip::<Opened>()
        .handler(copy_invoice())
        .handler(copy_credit_note())
        .handler(copy_discount())
        .handler(copy_shipping())
        .handler(copy_euro_rate())
        .strict()
}

impl IssuedDocument {
    /// The invoice as its `InvoiceIssued` says it, before the events written
    /// beside it (discount, shipping, euro rate) are applied.
    fn of_invoice(data: &InvoiceIssued) -> Self {
        Self {
            document_id: data.document_id.to_owned(),
            company_id: data.company_id.to_owned(),
            kind: DocumentKind::Invoice,
            series: data.series.to_owned(),
            number: data.number,
            label: document_label(&data.series, data.number),
            issued_at: data.issued_at,
            order_id: data.order_id.to_owned(),
            order_reference: data.order_reference.to_owned(),
            paid_at: Some(data.paid_at),
            payment_method: Some(data.payment_method.to_owned()),
            seller: data.seller.to_owned(),
            buyer: data.buyer.to_owned(),
            buyer_email: data.buyer_email.to_owned(),
            delivery: Some(data.delivery.to_owned()),
            currency: data.currency.to_owned(),
            lines: data.lines.to_owned(),
            vat: data.vat.to_owned(),
            total_excl_vat_cents: data.total_excl_vat_cents,
            total_vat_cents: data.total_vat_cents,
            total_cents: data.total_cents,
            template: data.template,
            regime: Regime::parse(&data.regime).unwrap_or_default(),
            tax_country: data.tax_country.to_owned(),
            buyer_vat_number: data.buyer_vat_number.to_owned(),
            vies_reference: data.vies_reference.to_owned(),
            eu_distance_sales_cents: data.eu_distance_sales_cents,
            eu_threshold_passed: data.eu_threshold_passed,
            ..Self::default()
        }
    }

    /// The credit note as its `CreditNoteIssued` says it, before the euro
    /// rate written beside it is applied.
    fn of_credit_note(data: &CreditNoteIssued) -> Self {
        Self {
            document_id: data.document_id.to_owned(),
            company_id: data.company_id.to_owned(),
            kind: DocumentKind::CreditNote,
            series: data.series.to_owned(),
            number: data.number,
            label: document_label(&data.series, data.number),
            issued_at: data.issued_at,
            order_id: data.order_id.to_owned(),
            order_reference: data.order_reference.to_owned(),
            invoice_document_id: Some(data.invoice_document_id.to_owned()),
            invoice_label: Some(data.invoice_label.to_owned()),
            refund_id: Some(data.refund_id.to_owned()),
            seller: data.seller.to_owned(),
            buyer: data.buyer.to_owned(),
            buyer_email: data.buyer_email.to_owned(),
            currency: data.currency.to_owned(),
            vat: data.vat.to_owned(),
            total_excl_vat_cents: data.total_excl_vat_cents,
            total_vat_cents: data.total_vat_cents,
            total_cents: data.total_cents,
            template: data.template,
            regime: Regime::parse(&data.regime).unwrap_or_default(),
            tax_country: data.tax_country.to_owned(),
            buyer_vat_number: data.buyer_vat_number.to_owned(),
            ..Self::default()
        }
    }

    fn apply_discount(&mut self, data: &InvoiceDiscounted) {
        self.discount = Some(InvoiceDiscount {
            code: data.code.to_owned(),
            amount_cents: data.amount_cents,
        });
    }

    fn apply_shipping(&mut self, data: &InvoiceShippingCharged) {
        self.shipping = Some(InvoiceShipping {
            name: data.name.to_owned(),
            amount_cents: data.amount_cents,
        });
    }

    fn apply_euro_rate(&mut self, data: &EuroRateRecorded) {
        self.euro_rate = Some(data.rate.to_owned());
        self.total_vat_eur_cents = Some(data.total_vat_eur_cents);
    }
}

#[evento::handler]
async fn copy_invoice(event: Event<InvoiceIssued>, book: &mut BookDocuments) -> anyhow::Result<()> {
    let data = &event.data;
    book.id = event.aggregate_id.to_owned();
    book.documents.insert(
        data.document_id.to_owned(),
        IssuedDocument::of_invoice(data),
    );
    Ok(())
}

#[evento::handler]
async fn copy_credit_note(
    event: Event<CreditNoteIssued>,
    book: &mut BookDocuments,
) -> anyhow::Result<()> {
    let data = &event.data;
    book.id = event.aggregate_id.to_owned();
    book.documents.insert(
        data.document_id.to_owned(),
        IssuedDocument::of_credit_note(data),
    );
    Ok(())
}

#[evento::handler]
async fn copy_discount(
    event: Event<InvoiceDiscounted>,
    book: &mut BookDocuments,
) -> anyhow::Result<()> {
    // Written right after the `InvoiceIssued` it belongs to.
    if let Some(document) = book.documents.get_mut(&event.data.document_id) {
        document.apply_discount(&event.data);
    }
    Ok(())
}

#[evento::handler]
async fn copy_shipping(
    event: Event<InvoiceShippingCharged>,
    book: &mut BookDocuments,
) -> anyhow::Result<()> {
    // Written right after the `InvoiceIssued` it belongs to.
    if let Some(document) = book.documents.get_mut(&event.data.document_id) {
        document.apply_shipping(&event.data);
    }
    Ok(())
}

#[evento::handler]
async fn copy_euro_rate(
    event: Event<EuroRateRecorded>,
    book: &mut BookDocuments,
) -> anyhow::Result<()> {
    // Written right after the `InvoiceIssued` or `CreditNoteIssued` it
    // belongs to.
    if let Some(document) = book.documents.get_mut(&event.data.document_id) {
        document.apply_euro_rate(&event.data);
    }
    Ok(())
}

/// Which chunk of a series is open.
#[evento::projection(id = id, name = "timada-invoicing/BookPointerState")]
#[evento::snapshot(none)]
#[derive(Debug, PartialEq, Eq)]
pub struct BookPointerState {
    pub id: String,
    pub chunk: u32,
}

pub fn book_pointer_projection<E: Executor>() -> Projection<E, BookPointerState> {
    Projection::new::<BookPointer>()
        .handler(on_rolled())
        .strict()
}

#[evento::handler]
async fn on_rolled(event: Event<Rolled>, pointer: &mut BookPointerState) -> anyhow::Result<()> {
    pointer.id = event.aggregate_id.to_owned();
    pointer.chunk = pointer.chunk.max(event.data.chunk);
    Ok(())
}

// =============================================================================
// Document (index)
// =============================================================================

#[evento::projection(id = id, name = "timada-invoicing/DocumentState")]
#[evento::snapshot(none)]
#[derive(Debug, PartialEq, Eq)]
pub struct DocumentState {
    pub id: String,
    /// The series it was reserved in.
    pub series: String,
    /// The calendar year of that series.
    pub year: u16,
    /// The chunk of the series open when it was reserved: it is in that
    /// one or a later one.
    pub chunk: u32,
    /// For an invoice, the chunk of the company's distance ledger open when
    /// it was reserved.
    pub ledger_chunk: Option<u32>,
    /// The book holding it, once recorded.
    pub book_id: Option<String>,
    /// The document itself, once recorded: what a page reads.
    pub document: Option<IssuedDocument>,
}

pub fn document_projection<E: Executor>() -> Projection<E, DocumentState> {
    Projection::new::<Document>()
        .handler(on_reserved())
        .handler(on_recorded())
        .strict()
}

#[evento::handler]
async fn on_reserved(event: Event<Reserved>, document: &mut DocumentState) -> anyhow::Result<()> {
    document.id = event.aggregate_id.to_owned();
    document.series = event.data.series.to_owned();
    document.year = event.data.year;
    document.chunk = event.data.chunk;
    document.ledger_chunk = event.data.ledger_chunk;
    Ok(())
}

#[evento::handler]
async fn on_recorded(event: Event<Recorded>, document: &mut DocumentState) -> anyhow::Result<()> {
    document.book_id = Some(event.data.book_id.to_owned());
    document.document = Some(event.data.document.to_owned());
    Ok(())
}

// =============================================================================
// Issuing
// =============================================================================

/// A line as it was sold, before its VAT is decided.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NewInvoiceLine {
    pub name: String,
    pub variant_label: Option<String>,
    pub sku: Option<String>,
    pub quantity: u32,
    /// The price of one, VAT included, in minor units.
    pub unit_amount_cents: u64,
    /// The catalog's tax category of the product (`standard`, `books`…).
    pub tax_category: String,
}

impl NewInvoiceLine {
    pub fn total_cents(&self) -> u64 {
        self.unit_amount_cents * u64::from(self.quantity)
    }
}

/// What an invoice says, gathered by the host when an order was paid. The
/// crate decides its VAT (see [`crate::tax`]).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NewInvoice {
    /// The company selling, and the tenant of it that sold.
    pub company_id: String,
    pub tenant_id: String,
    /// The tenant's series code in the company (`""` for the plain series).
    pub series_code: String,
    pub order_id: String,
    pub order_reference: String,
    pub paid_at: u64,
    pub payment_method: String,
    pub seller: Seller,
    pub buyer: Address,
    pub buyer_email: String,
    pub delivery: Address,
    pub currency: String,
    pub lines: Vec<NewInvoiceLine>,
    /// The discount code the order was paid with, if any.
    pub discount: Option<InvoiceDiscount>,
    /// The shipping the order was charged, if any.
    pub shipping: Option<InvoiceShipping>,
    /// What was paid: the lines, less the discount, plus shipping, must add
    /// up to it.
    pub total_cents: u64,
    pub issued_by: String,
    /// The seller as far as VAT goes.
    pub tax: TaxProfile,
    /// The buyer's EU VAT number, only once VIES said it is valid, with
    /// VIES's reference for the check.
    pub buyer_vat_number: Option<String>,
    pub vies_reference: Option<String>,
    /// The euro rate of `currency` on the day of issue, when it is not the
    /// euro: what the sale counts for against the yearly threshold of EU
    /// distance sales, and the VAT in euros the document also states. A
    /// distance sale from a seller without the One-Stop Shop cannot be
    /// issued without it ([`needs_euro_rate`]); any other sale is issued
    /// without, and then states no VAT in euros.
    pub euro_rate: Option<ExchangeRate>,
}

/// Money given back against an order's invoice.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NewCreditNote {
    pub order_id: String,
    /// The provider's id of the refund: one credit note per refund.
    pub refund_id: String,
    pub amount_cents: u64,
    pub issued_by: String,
}

/// The share of shipping each line carries: in proportion to what the line
/// was sold for, or evenly when the goods were all free.
pub fn shipping_shares(goods: &[u64], shipping_cents: u64) -> Vec<u64> {
    if goods.iter().any(|line| *line > 0) {
        spread(goods, shipping_cents)
    } else {
        spread(&vec![1; goods.len()], shipping_cents)
    }
}

/// The chunk of a series the next document goes in, as read for one
/// attempt.
struct OpenBook {
    /// The pointer, `None` while chunk 0 is open.
    pointer: Option<BookPointerState>,
    /// The chunk the pointer names (0 without one).
    chunk: u32,
    /// The stream of that chunk.
    id: String,
    /// Its state, `None` when nothing was ever written to it.
    state: Option<BookState>,
}

impl OpenBook {
    /// Whether nothing more may be written to it: the next write opens the
    /// chunk after it.
    fn full(&self) -> anyhow::Result<bool> {
        match &self.state {
            Some(book) => Ok(book.aggregate_version()? >= chunk_size()),
            None => Ok(false),
        }
    }

    /// The version a write to it is made at.
    fn version(&self) -> anyhow::Result<u16> {
        self.state
            .as_ref()
            .map_or(Ok(0), |book| book.aggregate_version())
    }

    /// The number the next document takes.
    fn next(&self) -> u64 {
        self.state.as_ref().map_or(0, |book| book.last_number) + 1
    }

    fn holds(&self, document_id: &str) -> bool {
        self.state
            .as_ref()
            .is_some_and(|book| book.documents.contains(document_id))
    }
}

impl<E: Executor> Command<E> {
    /// Issues the invoice of a paid order at `now` (Unix seconds); an order
    /// already invoiced returns its invoice unchanged. An EU distance sale
    /// is decided against its company's yearly total, kept in `ledger`
    /// (the default tenant of the store).
    pub async fn issue_invoice<P: Executor>(
        &self,
        invoice: NewInvoice,
        ledger: &Ledger<P>,
        now: u64,
    ) -> Result<IssuedDocument, InvoicingError> {
        if invoice.lines.is_empty() {
            return Err(InvoicingError::EmptyInvoice);
        }
        let gross: Vec<u64> = invoice
            .lines
            .iter()
            .map(NewInvoiceLine::total_cents)
            .collect();
        let discount_cents = invoice.discount.as_ref().map_or(0, |d| d.amount_cents);
        let shipping_cents = invoice.shipping.as_ref().map_or(0, |s| s.amount_cents);
        let total = gross
            .iter()
            .sum::<u64>()
            .checked_sub(discount_cents)
            .and_then(|goods| goods.checked_add(shipping_cents))
            .ok_or(InvoicingError::TotalMismatch)?;
        if total != invoice.total_cents {
            return Err(InvoicingError::TotalMismatch);
        }
        let destination = Destination {
            country: invoice.delivery.country.clone(),
            postal_code: invoice.delivery.postal_code.clone(),
            vat_number: invoice.buyer_vat_number.clone(),
        };
        // The rate of the invoice's own currency, parsed, or nothing.
        let euro_rate = invoice.euro_rate.clone().filter(|rate| {
            !invoice.currency.eq_ignore_ascii_case(EURO) && rate.converts(&invoice.currency)
        });
        if euro_rate.is_none() && needs_euro_rate(invoice.tax, &destination, &invoice.currency) {
            return Err(InvoicingError::EuroRateMissing {
                currency: invoice.currency.clone(),
            });
        }
        // What each line was really sold for: the discount spread over the
        // lines in proportion, to the cent. Shipping is an accessory of the
        // goods it carries: it is spread over them the same way and taxed
        // with them. The VAT and the distance sales are of these.
        let goods: Vec<u64> = gross
            .iter()
            .zip(spread(&gross, discount_cents))
            .map(|(line, off)| line - off)
            .collect();
        let net: Vec<u64> = goods
            .iter()
            .zip(shipping_shares(&goods, shipping_cents))
            .map(|(line, shipping)| line + shipping)
            .collect();
        let document_id = invoice_id(&invoice.order_id);
        let year = year_of(now);
        // Where a retry starts looking for its count: the chunk of the
        // ledger open before the reservation.
        let ledger_chunk = ledger
            .open_chunk_number(&invoice.company_id, i64::from(year))
            .await?;
        let index = self
            .reserve(
                &document_id,
                series_of(INVOICE_PREFIX, &invoice.series_code, now),
                year,
                Some(ledger_chunk),
            )
            .await?;
        if let Some(document) = index.document {
            return Ok(document);
        }

        // The VAT, fixed before the number: for a distance sale, counted
        // in the company's ledger (a retry gets the same decision back).
        let sold: Vec<(&str, u64)> = invoice
            .lines
            .iter()
            .zip(&net)
            .map(|(line, net)| (line.tax_category.as_str(), *net))
            .collect();
        let decide_with = |before: DistanceSales| {
            decide(
                invoice.tax,
                &destination,
                &sold,
                &invoice.currency,
                euro_rate.as_ref(),
                before,
            )
        };
        let decision = if is_distance_sale(invoice.tax, &destination) {
            ledger
                .count(
                    &invoice.company_id,
                    i64::from(index.year),
                    &document_id,
                    &invoice.tenant_id,
                    index.ledger_chunk.unwrap_or(0),
                    decide_with,
                )
                .await?
        } else {
            decide_with(DistanceSales::default())
        };

        let lines: Vec<InvoiceLine> = invoice
            .lines
            .iter()
            .zip(&decision.rates)
            .map(|(line, rate)| InvoiceLine {
                name: line.name.clone(),
                variant_label: line.variant_label.clone(),
                sku: line.sku.clone(),
                quantity: line.quantity,
                unit_amount_cents: line.unit_amount_cents,
                vat_rate_bps: *rate,
            })
            .collect();
        let vat = vat_breakdown(
            lines
                .iter()
                .zip(&net)
                .filter_map(|(line, net)| line.vat_rate_bps.map(|rate| (rate, *net))),
        );
        let total_vat_cents: u64 = vat.iter().map(|subtotal| subtotal.vat_cents).sum();
        let reverse_charge = decision.regime == Regime::ReverseCharge;
        // The rate the sale was counted at (a retry's is the ledger's), or
        // the day's for a sale that is not counted.
        let euro_rate = decision.euro_rate.clone().or(euro_rate);
        let in_euros = euro_rate
            .as_ref()
            .and_then(|rate| Some((rate.clone(), rate.to_euro_cents(total_vat_cents)?)));
        self.issue_reserved(&document_id, |book, series, number| {
            let mut write = evento::append(book);
            let issued = InvoiceIssued {
                document_id: document_id.clone(),
                series: series.to_owned(),
                number,
                issued_at: now,
                order_id: invoice.order_id.clone(),
                order_reference: invoice.order_reference.clone(),
                paid_at: invoice.paid_at,
                payment_method: invoice.payment_method.clone(),
                seller: invoice.seller.clone(),
                buyer: invoice.buyer.clone(),
                buyer_email: invoice.buyer_email.clone(),
                delivery: invoice.delivery.clone(),
                currency: invoice.currency.clone(),
                lines: lines.clone(),
                vat: vat.clone(),
                total_excl_vat_cents: total - total_vat_cents,
                total_vat_cents,
                total_cents: total,
                template: TEMPLATE_VERSION,
                issued_by: invoice.issued_by.clone(),
                company_id: invoice.company_id.clone(),
                regime: decision.regime.as_str().to_owned(),
                tax_country: decision.tax_country.clone(),
                buyer_vat_number: invoice.buyer_vat_number.clone().filter(|_| reverse_charge),
                vies_reference: invoice.vies_reference.clone().filter(|_| reverse_charge),
                eu_distance_sales_cents: decision.distance.sales_cents,
                eu_threshold_passed: decision.distance.passed,
            };
            let mut document = IssuedDocument::of_invoice(&issued);
            write.event(&issued);
            if let Some(discount) = &invoice.discount {
                let discounted = InvoiceDiscounted {
                    document_id: document_id.clone(),
                    code: discount.code.clone(),
                    amount_cents: discount.amount_cents,
                };
                document.apply_discount(&discounted);
                write.event(&discounted);
            }
            if let Some(shipping) = &invoice.shipping {
                let charged = InvoiceShippingCharged {
                    document_id: document_id.clone(),
                    name: shipping.name.clone(),
                    amount_cents: shipping.amount_cents,
                };
                document.apply_shipping(&charged);
                write.event(&charged);
            }
            if let Some((rate, total_vat_eur_cents)) = &in_euros {
                let recorded = EuroRateRecorded {
                    document_id: document_id.clone(),
                    rate: rate.clone(),
                    total_vat_eur_cents: *total_vat_eur_cents,
                };
                document.apply_euro_rate(&recorded);
                write.event(&recorded);
            }
            (write, document)
        })
        .await
    }

    /// Issues the credit note of a refund at `now`, against the order's
    /// invoice and in the credit-note series of the invoice's code; a
    /// refund already credited returns its credit note unchanged.
    pub async fn issue_credit_note(
        &self,
        credit: NewCreditNote,
        now: u64,
    ) -> Result<IssuedDocument, InvoicingError> {
        let invoice = self
            .load_invoice_of_order(&credit.order_id)
            .await?
            .ok_or(InvoicingError::InvoiceNotFound)?;
        if credit.amount_cents > invoice.total_cents {
            return Err(InvoicingError::CreditTooLarge);
        }
        let vat = prorate(&invoice.vat, credit.amount_cents);
        let total_vat_cents: u64 = vat.iter().map(|subtotal| subtotal.vat_cents).sum();
        // The VAT given back, in euros at the invoice's rate: the tax was
        // chargeable on the invoice's day.
        let in_euros = invoice
            .euro_rate
            .as_ref()
            .and_then(|rate| Some((rate.clone(), rate.to_euro_cents(total_vat_cents)?)));
        let document_id = credit_note_id(&credit.refund_id);
        let index = self
            .reserve(
                &document_id,
                series_of(CREDIT_NOTE_PREFIX, series_code(&invoice.series), now),
                year_of(now),
                None,
            )
            .await?;
        if let Some(document) = index.document {
            return Ok(document);
        }
        self.issue_reserved(&document_id, |book, series, number| {
            let mut write = evento::append(book);
            let issued = CreditNoteIssued {
                document_id: document_id.clone(),
                series: series.to_owned(),
                number,
                issued_at: now,
                invoice_document_id: invoice.document_id.clone(),
                invoice_label: invoice.label.clone(),
                order_id: invoice.order_id.clone(),
                order_reference: invoice.order_reference.clone(),
                refund_id: credit.refund_id.clone(),
                seller: invoice.seller.clone(),
                buyer: invoice.buyer.clone(),
                buyer_email: invoice.buyer_email.clone(),
                currency: invoice.currency.clone(),
                vat: vat.clone(),
                total_excl_vat_cents: credit.amount_cents - total_vat_cents,
                total_vat_cents,
                total_cents: credit.amount_cents,
                template: TEMPLATE_VERSION,
                issued_by: credit.issued_by.clone(),
                company_id: invoice.company_id.clone(),
                regime: invoice.regime.as_str().to_owned(),
                tax_country: invoice.tax_country.clone(),
                buyer_vat_number: invoice.buyer_vat_number.clone(),
            };
            let mut document = IssuedDocument::of_credit_note(&issued);
            write.event(&issued);
            if let Some((rate, total_vat_eur_cents)) = &in_euros {
                let recorded = EuroRateRecorded {
                    document_id: document_id.clone(),
                    rate: rate.clone(),
                    total_vat_eur_cents: *total_vat_eur_cents,
                };
                document.apply_euro_rate(&recorded);
                write.event(&recorded);
            }
            (write, document)
        })
        .await
    }

    /// Step 1 of the module doc: the document's index, reserved in `series`
    /// unless it was already (then in the series it was reserved in), noting
    /// the chunk of the series open now and `ledger_chunk`.
    async fn reserve(
        &self,
        document_id: &str,
        series: String,
        year: u16,
        ledger_chunk: Option<u32>,
    ) -> Result<DocumentState, InvoicingError> {
        for _ in 0..ATTEMPTS {
            if let Some(index) = document_projection()
                .load(document_id)
                .tenant(&self.tenant)
                .execute(&self.executor)
                .await?
            {
                return Ok(index);
            }
            // The document goes in this chunk or a later one: the pointer
            // only moves forward.
            let chunk = self.open_chunk_number(&series).await?;
            let reserved = evento::append(document_id)
                .original_version(0)
                .event(&Reserved {
                    series: series.clone(),
                    year,
                    chunk,
                    ledger_chunk,
                })
                .tenant(&self.tenant)
                .commit(&self.executor)
                .await;
            match reserved {
                Ok(_) | Err(WriteError::InvalidOriginalVersion) => continue,
                Err(err) => return Err(err.into()),
            }
        }
        Err(InvoicingError::Busy)
    }

    /// Steps 2 and 3 of the module doc for a reserved document. `event`
    /// builds the write of the document numbered `number` in `series` to
    /// the book stream it is given (the version is set here), and the
    /// document as that write says it.
    async fn issue_reserved<F>(
        &self,
        document_id: &str,
        event: F,
    ) -> Result<IssuedDocument, InvoicingError>
    where
        F: Fn(&str, &str, u64) -> (evento::WriteBuilder, IssuedDocument),
    {
        for _ in 0..ATTEMPTS {
            let Some(index) = document_projection()
                .load(document_id)
                .tenant(&self.tenant)
                .execute(&self.executor)
                .await?
            else {
                return Err(anyhow::anyhow!("document {document_id} is not reserved").into());
            };
            if let Some(document) = index.document {
                return Ok(document);
            }

            // A retry: the document may be in a chunk already.
            let open = self.open_book(&index.series).await?;
            if let Some((book, document)) = self.find_issued(&index, &open, document_id).await? {
                self.record(&index, &book, &document).await?;
                return Ok(document);
            }

            // 2. Issue.
            let (mut write, document) = event(&open.id, &index.series, open.next());
            let committed = write
                .original_version(open.version()?)
                .tenant(&self.tenant)
                .commit(&self.executor)
                .await;
            match committed {
                Ok(_) => {}
                Err(WriteError::InvalidOriginalVersion) => continue,
                Err(err) => return Err(err.into()),
            }

            // 3. Record.
            self.record(&index, &open.id, &document).await?;
            return Ok(document);
        }
        Err(InvoicingError::Busy)
    }

    /// Points the index at its book, the document copied beside; another
    /// writer having done it already is fine.
    async fn record(
        &self,
        index: &DocumentState,
        book_id: &str,
        document: &IssuedDocument,
    ) -> Result<(), InvoicingError> {
        let recorded = index
            .write()?
            .event(&Recorded {
                book_id: book_id.to_owned(),
                document: document.to_owned(),
            })
            .tenant(&self.tenant)
            .commit(&self.executor)
            .await;
        match recorded {
            Ok(_) | Err(WriteError::InvalidOriginalVersion) => Ok(()),
            Err(err) => Err(err.into()),
        }
    }

    /// The document, with the chunk holding it, when a writer put it in
    /// one: looked for from the chunk open when it was reserved to the open
    /// one (`open`, read already), reading a chunk's documents only once
    /// their ids say it is there.
    async fn find_issued(
        &self,
        index: &DocumentState,
        open: &OpenBook,
        document_id: &str,
    ) -> Result<Option<(String, IssuedDocument)>, InvoicingError> {
        for chunk in index.chunk..=open.chunk {
            let id = book_id(&index.series, chunk);
            let holds = if chunk == open.chunk {
                open.holds(document_id)
            } else {
                book_projection()
                    .load(&id)
                    .tenant(&self.tenant)
                    .execute(&self.executor)
                    .await?
                    .is_some_and(|book| book.documents.contains(document_id))
            };
            if !holds {
                continue;
            }
            let document = book_documents_projection()
                .load(&id)
                .tenant(&self.tenant)
                .execute(&self.executor)
                .await?
                .and_then(|book| book.documents.get(document_id).cloned())
                .ok_or_else(|| anyhow::anyhow!("document {document_id} is not in book {id}"))?;
            return Ok(Some((id, document)));
        }
        Ok(None)
    }

    /// The chunk of `series` the pointer names now: where a document
    /// reserved now is written, or a later one.
    async fn open_chunk_number(&self, series: &str) -> Result<u32, InvoicingError> {
        Ok(book_pointer_projection()
            .load(book_pointer_id(series))
            .tenant(&self.tenant)
            .execute(&self.executor)
            .await?
            .map_or(0, |pointer| pointer.chunk))
    }

    /// The chunk of `series` the next document goes in: the one the pointer
    /// names, rolled past while full.
    async fn open_book(&self, series: &str) -> Result<OpenBook, InvoicingError> {
        loop {
            let open = self.read_open_book(series).await?;
            if !open.full()? {
                return Ok(open);
            }
            self.roll(series, open).await?;
        }
    }

    /// The chunk the pointer names, as it is now.
    async fn read_open_book(&self, series: &str) -> Result<OpenBook, InvoicingError> {
        let pointer = book_pointer_projection()
            .load(book_pointer_id(series))
            .tenant(&self.tenant)
            .execute(&self.executor)
            .await?;
        let chunk = pointer.as_ref().map_or(0, |pointer| pointer.chunk);
        let id = book_id(series, chunk);
        let state = book_projection()
            .load(&id)
            .tenant(&self.tenant)
            .execute(&self.executor)
            .await?;
        if chunk > 0 && state.is_none() {
            // Its `Opened` is committed before the pointer names it: a read
            // that sees the one without the other is behind, and numbering
            // from nothing would start the chunk at 1.
            return Err(
                anyhow::anyhow!("chunk {chunk} of {series} is named but not opened").into(),
            );
        }
        Ok(OpenBook {
            pointer,
            chunk,
            id,
            state,
        })
    }

    /// Opens the chunk after the full one `open`, from the number it ended
    /// with, then points at it. Another writer doing the same at the same
    /// moment carries the same number, so either write losing its race is
    /// the work done.
    async fn roll(&self, series: &str, open: OpenBook) -> Result<(), InvoicingError> {
        let Some(full) = open.state else {
            return Ok(());
        };
        let next = open.chunk + 1;
        let opened = evento::append(book_id(series, next))
            .original_version(0)
            .event(&Opened {
                last_number: full.last_number,
            })
            .tenant(&self.tenant)
            .commit(&self.executor)
            .await;
        match opened {
            Ok(_) | Err(WriteError::InvalidOriginalVersion) => {}
            Err(err) => return Err(err.into()),
        }
        let rolled = Rolled { chunk: next };
        let pointed = match open.pointer {
            Some(pointer) => {
                pointer
                    .write()?
                    .event(&rolled)
                    .tenant(&self.tenant)
                    .commit(&self.executor)
                    .await
            }
            None => {
                evento::append(book_pointer_id(series))
                    .original_version(0)
                    .event(&rolled)
                    .tenant(&self.tenant)
                    .commit(&self.executor)
                    .await
            }
        };
        match pointed {
            Ok(_) | Err(WriteError::InvalidOriginalVersion) => Ok(()),
            Err(err) => Err(err.into()),
        }
    }

    /// A document by its index stream, once issued: read from that stream
    /// alone, never from its book.
    pub async fn load_document(
        &self,
        document_id: &str,
    ) -> Result<Option<IssuedDocument>, InvoicingError> {
        Ok(document_projection()
            .load(document_id)
            .tenant(&self.tenant)
            .execute(&self.executor)
            .await?
            .and_then(|index| index.document))
    }

    /// The invoice of an order, once issued.
    pub async fn load_invoice_of_order(
        &self,
        order_id: &str,
    ) -> Result<Option<IssuedDocument>, InvoicingError> {
        self.load_document(&invoice_id(order_id)).await
    }

    /// The credit note of a refund, once issued.
    pub async fn load_credit_note_of_refund(
        &self,
        refund_id: &str,
    ) -> Result<Option<IssuedDocument>, InvoicingError> {
        self.load_document(&credit_note_id(refund_id)).await
    }
}

#[cfg(test)]
mod tests;
