#[derive(Debug, thiserror::Error)]
pub enum ShippingError {
    #[error("shipment not found")]
    ShipmentNotFound,
    #[error("shipment has no lines")]
    NoLines,
    #[error("shipment is no longer awaiting dispatch")]
    NotCreated,
    #[error("shipment has not been dispatched")]
    NotDispatched,
    #[error("`{0}` is required")]
    Required(&'static str),
    #[error(transparent)]
    Address(#[from] timada_core::AddressError),
    #[error(transparent)]
    Write(#[from] evento::WriteError),
    #[error(transparent)]
    Server(#[from] anyhow::Error),
}
