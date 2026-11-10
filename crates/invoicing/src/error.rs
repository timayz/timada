use evento::WriteError;

#[derive(Debug, thiserror::Error)]
pub enum InvoicingError {
    #[error("an invoice needs at least one line")]
    EmptyInvoice,
    #[error("the lines do not add up to what was paid")]
    TotalMismatch,
    #[error("the order has no invoice yet")]
    InvoiceNotFound,
    #[error("a credit note gives back more than the invoice is for")]
    CreditTooLarge,
    #[error("no euro rate for {currency}: the sale cannot be counted against the EU threshold")]
    EuroRateMissing { currency: String },
    #[error("another node is writing to the same series; try again")]
    Busy,
    #[error(transparent)]
    Write(#[from] WriteError),
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

impl InvoicingError {
    /// Whether the failure is the store's rather than the document's: a
    /// write it is not taking for the moment, a series two writers kept
    /// racing on (`Busy`, or a version conflict past the retries), a read
    /// that failed. A saga fails its pass on these, and on these only; the
    /// rest (an empty invoice, lines that do not add up, a credit note too
    /// large, no euro rate for a sale that needs one) is the order's own and
    /// is recorded on it.
    pub fn is_store_fault(&self) -> bool {
        match self {
            Self::Write(err) => timada_core::is_store_write_fault(err),
            Self::Busy | Self::Other(_) => true,
            _ => false,
        }
    }
}
