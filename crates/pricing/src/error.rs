#[derive(Debug, thiserror::Error)]
pub enum PricingError {
    #[error("price not found")]
    PriceNotFound,
    #[error("product `{0}` already has a listed price")]
    AlreadyListed(String),
    #[error("price has been withdrawn")]
    PriceWithdrawn,
    #[error("amount must be positive")]
    InvalidAmount,
    #[error("amount must not be negative")]
    NegativeAmount,
    #[error("installment count must be between 2 and 4, got {0}")]
    InvalidInstallmentCount(u8),
    #[error("`{0}` is not a currency code")]
    InvalidCurrency(String),
    #[error("the product has no price in {0}")]
    NoPriceInCurrency(String),
    #[error("{0} is the currency the product is listed in: withdraw the price instead")]
    ListedCurrency(String),
    #[error("`{0}` is required")]
    Required(&'static str),
    #[error(transparent)]
    Money(#[from] timada_core::MoneyError),
    #[error(transparent)]
    Write(#[from] evento::WriteError),
    #[error(transparent)]
    Server(#[from] anyhow::Error),
}
