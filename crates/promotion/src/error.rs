use timada_core::Money;

#[derive(Debug, thiserror::Error)]
pub enum PromotionError {
    #[error("unknown code")]
    UnknownCode,
    #[error("code `{0}` already exists")]
    CodeAlreadyExists(String),
    #[error("discount is inactive")]
    Inactive,
    #[error("code has expired")]
    Expired,
    #[error("redemption limit reached")]
    LimitReached,
    #[error("voucher is cancelled")]
    Cancelled,
    #[error("insufficient voucher balance: {} {} remaining", remaining.minor, remaining.currency)]
    InsufficientBalance { remaining: Money },
    #[error("code takes nothing off this order")]
    NotApplicable,
    #[error("invalid discount kind")]
    InvalidKind,
    #[error("amount must be positive")]
    InvalidAmount,
    #[error("`{0}` is required")]
    Required(&'static str),
    #[error(transparent)]
    Money(#[from] timada_core::MoneyError),
    #[error(transparent)]
    Sql(#[from] sqlx::Error),
    #[error(transparent)]
    Write(#[from] evento::WriteError),
    #[error(transparent)]
    Server(#[from] anyhow::Error),
}
