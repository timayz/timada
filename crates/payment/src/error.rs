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
    #[error("refund not found")]
    RefundNotFound,
    #[error("refund has not failed")]
    RefundNotFailed,
    #[error("refund is already settled")]
    RefundAlreadySettled,
    #[error("payment provider: {0}")]
    Provider(#[from] crate::provider::ProviderError),
    #[error(transparent)]
    Sql(#[from] sqlx::Error),
    #[error(transparent)]
    Money(#[from] timada_core::MoneyError),
    #[error(transparent)]
    Write(#[from] evento::WriteError),
    #[error(transparent)]
    Server(#[from] anyhow::Error),
}
