#[derive(Debug, thiserror::Error)]
pub enum InventoryError {
    #[error("stock item not found")]
    StockItemNotFound,
    #[error("stock item already registered")]
    AlreadyRegistered,
    #[error("back-in-stock alert not found")]
    AlertNotFound,
    #[error("back-in-stock alert already requested")]
    AlreadyRequested,
    #[error("quantity must be greater than zero")]
    InvalidQuantity,
    #[error("`{0}` is required")]
    Required(&'static str),
    #[error(transparent)]
    Write(#[from] evento::WriteError),
    #[error(transparent)]
    Server(#[from] anyhow::Error),
}
