#[derive(Debug, thiserror::Error)]
pub enum InvoiceError {
    #[error("invoice not found")]
    InvoiceNotFound,
    #[error("invoice has no lines")]
    NoLines,
    #[error("invoice is voided")]
    InvoiceVoided,
    #[error("`{0}` is required")]
    Required(&'static str),
    #[error(transparent)]
    Money(#[from] timada_core::MoneyError),
    #[error(transparent)]
    Database(#[from] sqlx::Error),
    #[error(transparent)]
    Write(#[from] evento::WriteError),
    #[error(transparent)]
    Server(#[from] anyhow::Error),
}
