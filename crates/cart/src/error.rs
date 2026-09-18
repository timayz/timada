#[derive(Debug, thiserror::Error)]
pub enum CartError {
    #[error("cart not found")]
    CartNotFound,
    #[error("cart is already checked out")]
    CartAlreadyCheckedOut,
    #[error("cart is empty")]
    EmptyCart,
    #[error("a customer is required to check out")]
    CustomerRequired,
    #[error("quantity must be at least 1")]
    InvalidQuantity,
    #[error("installments must be paid in 2 to 4 times")]
    InvalidPaymentMode,
    #[error("product `{0}` is already in the cart")]
    LineAlreadyInCart(String),
    #[error("product `{0}` is not in the cart")]
    LineNotFound(String),
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
