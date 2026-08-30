//! What the invoice routers and subscriptions are wired with.

use timada_core::ServiceContext;

use crate::aggregate::Party;

/// Who is selling, and how the documents are numbered.
///
/// The seller is configuration, not domain state: it belongs to the business
/// running the shop, is the same on every invoice, and is copied onto each
/// `InvoiceIssued` so a change of address never rewrites history.
#[derive(Clone)]
pub struct InvoiceConfig {
    pub seller: Party,
    pub invoice_prefix: String,
    pub credit_note_prefix: String,
}

impl Default for InvoiceConfig {
    fn default() -> Self {
        Self {
            seller: Party::default(),
            invoice_prefix: "INV".to_owned(),
            credit_note_prefix: "CN".to_owned(),
        }
    }
}

impl InvoiceConfig {
    /// The default numbering with a real seller on it — the only thing most
    /// callers need to set.
    pub fn new(seller: Party) -> Self {
        Self {
            seller,
            ..Self::default()
        }
    }
}

#[derive(Clone)]
pub struct InvoiceState {
    pub ctx: ServiceContext,
    pub config: InvoiceConfig,
}
