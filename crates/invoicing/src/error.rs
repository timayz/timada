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
    #[error("another node is writing to the same series; try again")]
    Busy,
    #[error(transparent)]
    Write(#[from] WriteError),
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}
