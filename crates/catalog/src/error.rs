use evento::WriteError;

#[derive(Debug, thiserror::Error)]
pub enum CatalogError {
    #[error("the name must be between 1 and 120 characters")]
    InvalidName,
    #[error("a slug is 2 to 64 lowercase letters, digits and dashes")]
    InvalidSlug,
    #[error("this slug is already taken")]
    SlugTaken,
    #[error("the description must be at most 4000 characters")]
    InvalidDescription,
    #[error("the amount must be a positive number with at most two decimals")]
    InvalidAmount,
    #[error("unknown currency")]
    InvalidCurrency,
    #[error("product not found")]
    NotFound,
    #[error("this product is archived")]
    Archived,
    #[error("a product needs a price before it is published")]
    NotPriced,
    #[error(transparent)]
    Write(#[from] WriteError),
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

impl CatalogError {
    /// Whether the message can be shown to the person who made the request
    /// (as opposed to an infrastructure failure).
    pub fn is_user_facing(&self) -> bool {
        !matches!(self, Self::Write(_) | Self::Other(_))
    }
}
