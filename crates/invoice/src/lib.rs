//! Invoice bounded context: one invoice per order, drafted at placement,
//! issued (numbered from a write-side SQL sequence) once paid, voided when
//! the order is cancelled before that. An issued invoice is never edited:
//! every refund is documented by a credit note ("avoir") with its own number.

pub mod aggregator;
mod archive;
mod command;
mod credit_note_list;
mod document;
mod error;
mod invoice_list;
mod migration;
#[cfg(feature = "pdf")]
mod pdf;
mod process;
mod query;
mod value_object;
mod vat_journal;

pub use archive::*;
pub use command::*;
pub use credit_note_list::*;
pub use document::*;
pub use error::*;
pub use invoice_list::*;
pub use migration::migrations;
#[cfg(feature = "pdf")]
pub use pdf::*;
pub use process::*;
pub use query::*;
pub use value_object::*;
pub use vat_journal::*;
