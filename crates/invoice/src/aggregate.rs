//! The `Invoice` aggregate — one invoice per order, and its reversal.
//!
//! Two events, and the second one can only ever follow the first. There is no
//! "invoice corrected" event and there never will be: a document that has been
//! sent to a customer and booked into the accounts is not editable, so an
//! invoice that turns out to be wrong is cancelled by a credit note and, if
//! the sale still stands, re-issued as a new invoice with a new number.
//!
//! Everything the document shows is copied onto `InvoiceIssued` rather than
//! pointed at. That is the whole point of the aggregate — the order it invoices
//! is free to change hands, the seller free to move office and the VAT rate
//! free to rise, and the invoice still prints the deal as it was.
//!
//! There is no issue-date field: the date *is* the event's own timestamp, which
//! evento already records, and a second copy could only ever disagree with it.

use timada_core::Money;

/// One side of the transaction, frozen at issuance.
///
/// Flat and unvalidated for the same reason [`timada_order::Address`] is:
/// address formats are country-specific. `email` is a plain `String` and is
/// routinely empty on the seller — a company address is what a paper invoice
/// needs, an inbox is not.
#[derive(Debug, Clone, PartialEq, Eq, Default, bitcode::Encode, bitcode::Decode)]
pub struct Party {
    pub name: String,
    pub street: String,
    pub city: String,
    pub postal_code: String,
    pub country: String,
    pub email: String,
}

/// One billed line, with its tax worked out.
///
/// `unit_price_gross` is tax-inclusive, as everywhere in Timada, so
/// `net + tax == gross == unit_price_gross × quantity`. `gross` is carried
/// even though it is derivable: an invoice is a document, and every figure
/// printed on it has to be the figure that was printed on it, not one a later
/// rounding rule recomputes.
#[derive(Debug, Clone, PartialEq, Eq, Default, bitcode::Encode, bitcode::Decode)]
pub struct InvoiceLine {
    pub description: String,
    pub quantity: u32,
    pub unit_price_gross: Money,
    /// Rate applied to this line, in basis points (2000 = 20 %).
    pub tax_rate_bps: u32,
    /// This line's share of the order's discount; zero when none. `gross` is
    /// the discounted amount: `net + tax == gross == unit × qty − discount`.
    pub discount: Money,
    pub net: Money,
    pub tax: Money,
    pub gross: Money,
}

#[evento::aggregate]
pub enum Invoice {
    /// The order was paid, so there is something to bill for.
    ///
    /// Issued automatically by the `invoice-issuance` subscription; nothing
    /// else writes this event.
    InvoiceIssued {
        order_id: String,
        invoice_number: String,
        seller: Party,
        buyer: Party,
        lines: Vec<InvoiceLine>,
        /// The code the customer redeemed, and what it took off — printed on
        /// the document so the arithmetic on paper adds up.
        discount_code: Option<String>,
        discount_amount: Option<Money>,
        total_net: Money,
        total_tax: Money,
        total_gross: Money,
    },
    /// The sale came undone after it was invoiced — a supplier refused, the
    /// charge was refunded, the order cancelled. The invoice stays exactly as
    /// it was; this reverses it in full.
    CreditNoteIssued {
        credit_note_number: String,
        reason: String,
    },
}
