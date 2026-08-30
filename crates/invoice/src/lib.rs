//! Timada invoicing — the accounting record of what was sold.
//!
//! Nobody issues an invoice by hand. The `invoice-issuance` subscription in
//! [`subscriptions`](crate::subscriptions) watches the order context: an order
//! that reaches `OrderPaid` gets an [`InvoiceIssued`], and a paid order that is
//! later cancelled — the refund path — gets a [`CreditNoteIssued`] reversing
//! it. The order context knows nothing about any of this.
//!
//! An invoice is a *frozen* document. Everything it quotes — the parties, the
//! lines, the rates, the totals — is copied onto the event at issuance, so
//! reprinting it years later shows the deal as it stood on the day of the sale
//! and not whatever the catalog, the tax table or the customer's address say
//! now. That is also why an invoice is never edited: a mistake is reversed by a
//! credit note, never overwritten.
//!
//! Numbers are sequential per document kind, allocated from a SQL counter —
//! see [`numbering`](crate::numbering) for what that does and does not
//! guarantee.
//!
//! Reads split the usual way. The printable document replays
//! [`load_invoice`] through the `Rw` executor, so a customer who follows the
//! link the moment the order is paid sees the invoice that was just written.
//! The admin list reads the eventually-consistent `admin_invoice_list` table.

mod aggregate;
mod commands;
mod migrations;
mod numbering;
mod projections;
mod routes;
mod state;
mod subscriptions;
mod view;

pub use aggregate::{CreditNoteIssued, Invoice, InvoiceIssued, InvoiceLine, Party};
pub use commands::{invoice_id, issue_credit_note, issue_invoice};
pub use migrations::migrations;
pub use projections::{
    ADMIN_SUBSCRIPTION, AdminInvoiceRow, admin_subscription, recent_invoices, start_subscriptions,
};
pub use routes::{admin_router, store_router};
pub use state::{InvoiceConfig, InvoiceState};
pub use subscriptions::{ISSUANCE_SUBSCRIPTION, issuance_subscription};
pub use view::{InvoiceView, load_invoice};
