//! Write side: the command gateway, the strict write models it loads, and
//! the issuing of documents.
//!
//! Issuing a document goes in three steps, each safe to repeat:
//!
//! 1. *Reserve*: the document's index stream records the series it goes in
//!    (version 0, so one writer wins). A retry, even in another year, keeps
//!    looking in that series.
//! 2. *Issue*: the document is appended to the open book of the series with
//!    the version the book was read at, its number one more than the last.
//!    Two writers racing for a number: one append fails and reads again.
//!    Before taking a number, the open book and the one before are searched
//!    for the document, so a retry after a crash between steps 2 and 3
//!    never takes a second one.
//! 3. *Record*: the index points at the book.

mod ledger;

pub use ledger::*;

use std::collections::BTreeMap;
use std::ops::Deref;

use evento::{Executor, Projection, ProjectionAggregate, WriteError, metadata::Event};

use crate::aggregator::{CreditNoteIssued, InvoiceDiscounted, InvoiceIssued, Recorded, Reserved};
use crate::error::InvoicingError;
use crate::tax::{Destination, DistanceSales, Regime, TaxProfile, decide, is_distance_sale};
use crate::value_object::{
    Address, BOOK_CHUNK, CREDIT_NOTE_PREFIX, INVOICE_PREFIX, InvoiceLine, Seller, TEMPLATE_VERSION,
    VatSubtotal, book_id, credit_note_id, document_label, invoice_id, prorate, series_code,
    series_of, spread, vat_breakdown, year_of,
};

/// How often a write lost to another writer is tried again.
const ATTEMPTS: usize = 10;

/// How many documents a book holds: [`BOOK_CHUNK`], or three in the unit
/// tests so they can fill books.
const fn chunk_size() -> u16 {
    if cfg!(test) { 3 } else { BOOK_CHUNK }
}

/// The command gateway over a tenant's store.
pub struct Command<E: Executor>(pub E);

impl<E: Executor> Deref for Command<E> {
    type Target = E;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

// =============================================================================
// Documents
// =============================================================================

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum DocumentKind {
    #[default]
    Invoice,
    CreditNote,
}

impl DocumentKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Invoice => "invoice",
            Self::CreditNote => "credit_note",
        }
    }
}

/// A discount code taken off an invoice's lines.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InvoiceDiscount {
    pub code: String,
    /// VAT included, in minor units.
    pub amount_cents: u64,
}

/// An issued document as it reads: an invoice or a credit note (the fields
/// of the other kind are empty).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IssuedDocument {
    pub document_id: String,
    /// The company that issued it.
    pub company_id: String,
    pub kind: DocumentKind,
    pub series: String,
    pub number: u64,
    /// `F2026-000042`.
    pub label: String,
    pub issued_at: u64,
    pub order_id: String,
    pub order_reference: String,
    /// Invoice: when and how it was paid.
    pub paid_at: Option<u64>,
    pub payment_method: Option<String>,
    /// Credit note: the invoice it corrects and the refund it records.
    pub invoice_document_id: Option<String>,
    pub invoice_label: Option<String>,
    pub refund_id: Option<String>,
    pub seller: Seller,
    pub buyer: Address,
    pub buyer_email: String,
    /// Invoice: where the goods go.
    pub delivery: Option<Address>,
    pub currency: String,
    /// Invoice: what was sold, at full price.
    pub lines: Vec<InvoiceLine>,
    /// Invoice: the discount code taken off the lines, if any; `vat` and
    /// the totals are of what was paid.
    pub discount: Option<InvoiceDiscount>,
    pub vat: Vec<VatSubtotal>,
    pub total_excl_vat_cents: u64,
    pub total_vat_cents: u64,
    pub total_cents: u64,
    pub template: u16,
    /// How it is taxed.
    pub regime: Regime,
    /// The country whose VAT applies, or where the goods went.
    pub tax_country: String,
    /// The buyer's EU VAT number, when it bought as a business.
    pub buyer_vat_number: Option<String>,
    /// Invoice: VIES's reference for the check of that number.
    pub vies_reference: Option<String>,
    /// Invoice: the year's EU distance sales, this one included.
    pub eu_distance_sales_cents: u64,
    pub eu_threshold_passed: bool,
}

// =============================================================================
// Book
// =============================================================================

#[evento::projection(id = id, name = "timada-invoicing/BookState")]
#[evento::snapshot(none)]
#[derive(Debug, PartialEq, Eq)]
pub struct BookState {
    pub id: String,
    /// The number of the last document; `0` for a book with none.
    pub last_number: u64,
    /// Its documents, by index stream.
    pub documents: BTreeMap<String, IssuedDocument>,
}

pub fn book_projection<E: Executor>() -> Projection<E, BookState> {
    Projection::new::<crate::aggregator::Book>()
        .handler(on_invoice_issued())
        .handler(on_credit_note_issued())
        .handler(on_invoice_discounted())
        .strict()
}

#[evento::handler]
async fn on_invoice_issued(
    event: Event<InvoiceIssued>,
    book: &mut BookState,
) -> anyhow::Result<()> {
    let data = &event.data;
    book.id = event.aggregate_id.to_owned();
    book.last_number = data.number;
    book.documents.insert(
        data.document_id.to_owned(),
        IssuedDocument {
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
            ..IssuedDocument::default()
        },
    );
    Ok(())
}

#[evento::handler]
async fn on_invoice_discounted(
    event: Event<InvoiceDiscounted>,
    book: &mut BookState,
) -> anyhow::Result<()> {
    // Written right after the `InvoiceIssued` it belongs to.
    if let Some(document) = book.documents.get_mut(&event.data.document_id) {
        document.discount = Some(InvoiceDiscount {
            code: event.data.code.to_owned(),
            amount_cents: event.data.amount_cents,
        });
    }
    Ok(())
}

#[evento::handler]
async fn on_credit_note_issued(
    event: Event<CreditNoteIssued>,
    book: &mut BookState,
) -> anyhow::Result<()> {
    let data = &event.data;
    book.id = event.aggregate_id.to_owned();
    book.last_number = data.number;
    book.documents.insert(
        data.document_id.to_owned(),
        IssuedDocument {
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
            ..IssuedDocument::default()
        },
    );
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
    /// The book holding it, once recorded.
    pub book_id: Option<String>,
    pub label: Option<String>,
}

pub fn document_projection<E: Executor>() -> Projection<E, DocumentState> {
    Projection::new::<crate::aggregator::Document>()
        .handler(on_reserved())
        .handler(on_recorded())
        .strict()
}

#[evento::handler]
async fn on_reserved(event: Event<Reserved>, document: &mut DocumentState) -> anyhow::Result<()> {
    document.id = event.aggregate_id.to_owned();
    document.series = event.data.series.to_owned();
    document.year = event.data.year;
    Ok(())
}

#[evento::handler]
async fn on_recorded(event: Event<Recorded>, document: &mut DocumentState) -> anyhow::Result<()> {
    document.book_id = Some(event.data.book_id.to_owned());
    document.label = Some(event.data.label.to_owned());
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
    /// What was paid: the lines, less the discount, must add up to it.
    pub total_cents: u64,
    pub issued_by: String,
    /// The seller as far as VAT goes.
    pub tax: TaxProfile,
    /// The buyer's EU VAT number, only once VIES said it is valid, with
    /// VIES's reference for the check.
    pub buyer_vat_number: Option<String>,
    pub vies_reference: Option<String>,
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

/// The book a document goes in, as read for one attempt.
struct OpenBook {
    id: String,
    state: Option<BookState>,
    /// The number the next document takes.
    next: u64,
}

impl<E: Executor> Command<E> {
    /// Issues the invoice of a paid order at `now` (Unix seconds); an order
    /// already invoiced returns its invoice unchanged. An EU distance sale
    /// is decided against its company's yearly total, kept in `ledger`
    /// (the platform store).
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
        let total = gross
            .iter()
            .sum::<u64>()
            .checked_sub(discount_cents)
            .ok_or(InvoicingError::TotalMismatch)?;
        if total != invoice.total_cents {
            return Err(InvoicingError::TotalMismatch);
        }
        // What each line was really sold for: the discount spread over the
        // lines in proportion, to the cent. The VAT and the distance sales
        // are of these.
        let net: Vec<u64> = gross
            .iter()
            .zip(spread(&gross, discount_cents))
            .map(|(line, off)| line - off)
            .collect();
        let document_id = invoice_id(&invoice.order_id);
        let index = self
            .reserve(
                &document_id,
                series_of(INVOICE_PREFIX, &invoice.series_code, now),
                year_of(now),
            )
            .await?;
        if let Some(book) = &index.book_id {
            return self.document_in(book, &document_id).await;
        }

        // The VAT, fixed before the number: for a distance sale, counted
        // in the company's ledger (a retry gets the same decision back).
        let destination = Destination {
            country: invoice.delivery.country.clone(),
            postal_code: invoice.delivery.postal_code.clone(),
            vat_number: invoice.buyer_vat_number.clone(),
        };
        let sold: Vec<(&str, u64)> = invoice
            .lines
            .iter()
            .zip(&net)
            .map(|(line, net)| (line.tax_category.as_str(), *net))
            .collect();
        let decide_with = |before: DistanceSales| {
            decide(invoice.tax, &destination, &sold, &invoice.currency, before)
        };
        let decision = if is_distance_sale(invoice.tax, &destination) {
            ledger
                .count(
                    &invoice.company_id,
                    i64::from(index.year),
                    &document_id,
                    &invoice.tenant_id,
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
        self.issue_reserved(&document_id, |book, series, number| {
            let mut write = evento::append(book);
            write.event(&InvoiceIssued {
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
            });
            if let Some(discount) = &invoice.discount {
                write.event(&InvoiceDiscounted {
                    document_id: document_id.clone(),
                    code: discount.code.clone(),
                    amount_cents: discount.amount_cents,
                });
            }
            write
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
        let document_id = credit_note_id(&credit.refund_id);
        let index = self
            .reserve(
                &document_id,
                series_of(CREDIT_NOTE_PREFIX, series_code(&invoice.series), now),
                year_of(now),
            )
            .await?;
        if let Some(book) = &index.book_id {
            return self.document_in(book, &document_id).await;
        }
        self.issue_reserved(&document_id, |book, series, number| {
            evento::append(book)
                .event(&CreditNoteIssued {
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
                })
                .to_owned()
        })
        .await
    }

    /// Step 1 of the module doc: the document's index, reserved in `series`
    /// unless it was already (then in the series it was reserved in).
    async fn reserve(
        &self,
        document_id: &str,
        series: String,
        year: u16,
    ) -> Result<DocumentState, InvoicingError> {
        for _ in 0..ATTEMPTS {
            if let Some(index) = document_projection()
                .load(document_id)
                .execute(&self.0)
                .await?
            {
                return Ok(index);
            }
            let reserved = evento::append(document_id)
                .original_version(0)
                .event(&Reserved {
                    series: series.clone(),
                    year,
                })
                .commit(&self.0)
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
    /// the book stream it is given (the version is set here).
    async fn issue_reserved<F>(
        &self,
        document_id: &str,
        event: F,
    ) -> Result<IssuedDocument, InvoicingError>
    where
        F: Fn(&str, &str, u64) -> evento::WriteBuilder,
    {
        for _ in 0..ATTEMPTS {
            let Some(index) = document_projection()
                .load(document_id)
                .execute(&self.0)
                .await?
            else {
                return Err(anyhow::anyhow!("document {document_id} is not reserved").into());
            };
            if let Some(book) = &index.book_id {
                return self.document_in(book, document_id).await;
            }

            // A retry: the document may be in the book already.
            let (open, previous) = self.open_book(&index.series).await?;
            let issued = [open.state.as_ref(), previous.as_ref()]
                .into_iter()
                .flatten()
                .find_map(|book| {
                    book.documents
                        .get(document_id)
                        .map(|document| (book.id.clone(), document.clone()))
                });
            if let Some((book, document)) = issued {
                self.record(&index, &book, &document.label).await?;
                return Ok(document);
            }

            // 2. Issue.
            let committed = event(&open.id, &index.series, open.next)
                .original_version(
                    open.state
                        .as_ref()
                        .map_or(Ok(0), |book| book.aggregate_version())?,
                )
                .commit(&self.0)
                .await;
            match committed {
                Ok(_) => {}
                Err(WriteError::InvalidOriginalVersion) => continue,
                Err(err) => return Err(err.into()),
            }

            // 3. Record.
            let label = document_label(&index.series, open.next);
            self.record(&index, &open.id, &label).await?;
            return self.document_in(&open.id, document_id).await;
        }
        Err(InvoicingError::Busy)
    }

    /// Points the index at its book; another writer having done it already
    /// is fine.
    async fn record(
        &self,
        index: &DocumentState,
        book_id: &str,
        label: &str,
    ) -> Result<(), InvoicingError> {
        let recorded = index
            .write()?
            .event(&Recorded {
                book_id: book_id.to_owned(),
                label: label.to_owned(),
            })
            .commit(&self.0)
            .await;
        match recorded {
            Ok(_) | Err(WriteError::InvalidOriginalVersion) => Ok(()),
            Err(err) => Err(err.into()),
        }
    }

    /// The book of `series` the next document goes in, and the full one
    /// before it if any.
    async fn open_book(
        &self,
        series: &str,
    ) -> Result<(OpenBook, Option<BookState>), InvoicingError> {
        let mut previous: Option<BookState> = None;
        let mut chunk = 0;
        loop {
            let id = book_id(series, chunk);
            let state = book_projection().load(&id).execute(&self.0).await?;
            let full = match &state {
                Some(book) => book.aggregate_version()? >= chunk_size(),
                None => false,
            };
            if !full {
                let next = state
                    .as_ref()
                    .or(previous.as_ref())
                    .map_or(0, |book| book.last_number)
                    + 1;
                return Ok((OpenBook { id, state, next }, previous));
            }
            previous = state;
            chunk += 1;
        }
    }

    async fn document_in(
        &self,
        book_id: &str,
        document_id: &str,
    ) -> Result<IssuedDocument, InvoicingError> {
        book_projection()
            .load(book_id)
            .execute(&self.0)
            .await?
            .and_then(|mut book| book.documents.remove(document_id))
            .ok_or_else(|| {
                anyhow::anyhow!("document {document_id} is not in book {book_id}").into()
            })
    }

    /// A document by its index stream, once issued.
    pub async fn load_document(
        &self,
        document_id: &str,
    ) -> Result<Option<IssuedDocument>, InvoicingError> {
        let Some(book) = document_projection()
            .load(document_id)
            .execute(&self.0)
            .await?
            .and_then(|index| index.book_id)
        else {
            return Ok(None);
        };
        self.document_in(&book, document_id).await.map(Some)
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
