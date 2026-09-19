#[derive(Debug, thiserror::Error)]
pub enum ReturnError {
    #[error("return not found")]
    ReturnNotFound,
    #[error("order not found")]
    OrderNotFound,
    #[error("only a shipped order can be returned")]
    OrderNotShipped,
    #[error("the return window of this order is closed")]
    WindowClosed,
    #[error("nothing to return")]
    NoLines,
    #[error("product `{0}` is not part of the order")]
    UnknownLine(String),
    #[error("only {returnable} unit(s) of `{product_id}` can still be returned")]
    QuantityExceeded { product_id: String, returnable: u32 },
    #[error("return is not {expected} (it is {actual})")]
    WrongStatus {
        expected: &'static str,
        actual: &'static str,
    },
    #[error("cannot accept more units of `{0}` than were requested")]
    AcceptedExceedsRequested(String),
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
