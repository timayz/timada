#[derive(Debug, thiserror::Error)]
pub enum PaymentError {
    #[error("payment not found")]
    PaymentNotFound,
    #[error("amount must be positive")]
    InvalidAmount,
    #[error("invalid payment method: {0}")]
    InvalidMethod(&'static str),
    #[error("payment is not in the requested state")]
    NotRequested,
    #[error("payment has not been captured")]
    NotCaptured,
    #[error("refund exceeds the captured amount")]
    RefundExceedsCapture,
    #[error(transparent)]
    Money(#[from] timada_core::MoneyError),
    #[error(transparent)]
    Write(#[from] evento::WriteError),
    #[error(transparent)]
    Server(#[from] anyhow::Error),
}
