#[derive(Debug, thiserror::Error)]
pub enum OrderError {
    #[error("order not found")]
    OrderNotFound,
    #[error("an order for cart `{0}` already exists")]
    AlreadyPlaced(String),
    #[error("order has no lines")]
    NoLines,
    #[error("order is cancelled")]
    Cancelled,
    #[error("order is not {expected} (it is {actual})")]
    WrongStatus {
        expected: &'static str,
        actual: &'static str,
    },
    #[error("order still has {} {} to pay", due.minor, due.currency)]
    AmountDue { due: timada_core::Money },
    #[error("discount must be positive and at most {} {}", max.minor, max.currency)]
    InvalidDiscount { max: timada_core::Money },
    #[error("unknown delivery method `{0}`")]
    UnknownDeliveryMethod(String),
    #[error("`{0}` is required")]
    Required(&'static str),
    #[error(transparent)]
    Money(#[from] timada_core::MoneyError),
    #[error(transparent)]
    Address(#[from] timada_core::AddressError),
    #[error(transparent)]
    Write(#[from] evento::WriteError),
    #[error(transparent)]
    Server(#[from] anyhow::Error),
}
