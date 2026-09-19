pub mod credit_note_details;
pub mod invoice_details;

pub use credit_note_details::{CreditNoteView, load as load_credit_note};
pub use invoice_details::{InvoiceView, load as load_invoice};
