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
    #[error("category not found")]
    CategoryNotFound,
    #[error("this category is archived")]
    CategoryArchived,
    #[error("a category cannot be moved under one of its own sub-categories")]
    CategoryCycle,
    #[error("categories nest at most three levels deep")]
    CategoryTooDeep,
    #[error("a product belongs to at most ten categories")]
    TooManyCategories,
    #[error("a product carries at most twenty pictures")]
    TooManyImages,
    #[error("this is not a picture of a kind the shop accepts")]
    InvalidImage,
    #[error("the text describing a picture is at most 200 characters")]
    InvalidAlt,
    #[error("picture not found")]
    ImageNotFound,
    #[error("the new order must list every picture of the product exactly once")]
    InvalidImageOrder,
    #[error("a product varies in at most three ways")]
    TooManyOptions,
    #[error("an option's name must be between 1 and 40 characters")]
    InvalidOptionName,
    #[error("this option is already there")]
    OptionExists,
    #[error("an option takes 1 to 20 values, each at most 40 characters")]
    InvalidOptionValues,
    #[error("option not found")]
    OptionNotFound,
    #[error("the options cannot change while the product has variants")]
    OptionInUse,
    #[error("a variant still uses one of the values being dropped")]
    OptionValueInUse,
    #[error("a product sells at most a hundred variants")]
    TooManyVariants,
    #[error("a variant names a value for every option, exactly once")]
    InvalidSelection,
    #[error("this combination is already sold as another variant")]
    VariantExists,
    #[error("variant not found")]
    VariantNotFound,
    #[error("every price of a product is in the same currency")]
    CurrencyMismatch,
    #[error("a code is 1 to 64 of letters, digits, dot, dash and underscore")]
    InvalidSku,
    #[error("another variant already uses this code")]
    SkuTaken,
    #[error("a fact needs a name of at most 40 characters and a value of at most 400")]
    InvalidAttribute,
    #[error("a product's table holds at most thirty facts")]
    TooManyAttributes,
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
