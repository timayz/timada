use evento::WriteError;

#[derive(Debug, thiserror::Error)]
pub enum AliexpressError {
    #[error("give an AliExpress product number or its address")]
    InvalidProductId,
    #[error("this product is already imported")]
    AlreadyLinked,
    #[error("this product is being imported")]
    Importing,
    #[error("not imported from AliExpress")]
    NotFound,
    #[error("a price rule multiplies by 1 to 100 and adds at most 10,000")]
    InvalidPricingRule,
    #[error("AliExpress no longer offers this combination")]
    CombinationNotFound,
    #[error(transparent)]
    Write(#[from] WriteError),
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

impl AliexpressError {
    /// Whether the message can be shown to the person who made the request.
    pub fn is_user_facing(&self) -> bool {
        !matches!(self, Self::Write(_) | Self::Other(_))
    }

    /// A stable code for a page to carry in a URL; `None` for an
    /// infrastructure failure.
    pub fn code(&self) -> Option<&'static str> {
        Some(match self {
            Self::InvalidProductId => "invalid_aliexpress_product",
            Self::AlreadyLinked => "aliexpress_already_linked",
            Self::Importing => "aliexpress_importing",
            Self::NotFound => "aliexpress_not_found",
            Self::InvalidPricingRule => "invalid_pricing_rule",
            Self::CombinationNotFound => "aliexpress_combination_not_found",
            Self::Write(_) | Self::Other(_) => return None,
        })
    }
}
