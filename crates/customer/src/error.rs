#[derive(Debug, thiserror::Error)]
pub enum CustomerError {
    #[error("customer not found")]
    CustomerNotFound,
    #[error("delivery address not found")]
    AddressNotFound,
    #[error("the preferred delivery address cannot be removed while others remain")]
    CannotRemovePreferred,
    #[error("`{0}` is not a valid email address")]
    InvalidEmail(String),
    #[error("the customer has no company identity")]
    NoCompanyIdentity,
    #[error("the check is about another VAT number than the company's")]
    VatNumberMismatch,
    #[error("`{0}` is required")]
    Required(&'static str),
    #[error(transparent)]
    Address(#[from] timada_core::AddressError),
    #[error(transparent)]
    Write(#[from] evento::WriteError),
    #[error(transparent)]
    Server(#[from] anyhow::Error),
}
