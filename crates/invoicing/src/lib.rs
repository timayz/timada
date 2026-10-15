//! Invoicing bounded context: the invoices a merchant issues for the orders
//! it was paid, and the credit notes for the money it gave back, numbered
//! the way French law asks (CGI art. 242 nonies A): one continuous series
//! per year and kind, with no gap and in the order they were issued.
//!
//! Lives in a tenant's own event store next to the orders. A document is
//! written whole, once, into the stream of its series (a *book*): appending
//! with the version the book was read at is what makes a number unique and
//! the series gapless, and the document being the very event that takes the
//! number means a number is never taken without its document. A book stream
//! holds at most [`BOOK_CHUNK`] documents and the series carries on in the
//! next one (evento versions are 16 bits). A small index stream per
//! document ([`invoice_id`], [`credit_note_id`]) points at the book that
//! holds it, so a page finds it without a read model.
//!
//! A document copies everything it says (the seller as it was, the buyer,
//! the lines, the VAT): it never changes once issued and renders the same
//! forever, which is how it is kept for the ten years the law asks. The
//! crate is pure domain code: gathering what a document says is the host's.

pub mod aggregator;
mod command;
mod error;
#[cfg(feature = "pdf")]
pub mod pdf;
mod query;
pub mod tax;
mod value_object;

pub use command::*;
pub use error::*;
pub use query::*;
pub use value_object::*;
