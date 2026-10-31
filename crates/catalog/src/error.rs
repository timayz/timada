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
    #[error("unknown tax category")]
    InvalidTaxCategory,
    #[error("a stock is a whole number between 0 and 1,000,000")]
    InvalidStock,
    #[error("only {available} left")]
    OutOfStock {
        product_id: String,
        variant_id: Option<String>,
        available: u32,
    },
    #[error(transparent)]
    Write(#[from] WriteError),
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

impl CatalogError {
    /// Whether the message can be shown to the person who made the request
    /// (as opposed to an infrastructure failure).
    pub fn is_user_facing(&self) -> bool {
        match self {
            // A write the store is not taking right now is told to the person.
            Self::Write(err) => timada_core::write_refusal(err).is_some(),
            Self::Other(_) => false,
            _ => true,
        }
    }

    /// A stable code for the refusal, for a page or a storefront to word in
    /// its own language; `None` for an infrastructure failure. A category
    /// page rewords the codes a category shares with a product.
    pub fn code(&self) -> Option<&'static str> {
        Some(match self {
            Self::InvalidName => "invalid_product_name",
            Self::InvalidSlug => "invalid_product_slug",
            Self::SlugTaken => "product_slug_taken",
            Self::InvalidDescription => "invalid_description",
            Self::InvalidAmount => "invalid_amount",
            Self::InvalidCurrency => "invalid_currency",
            Self::NotFound => "product_not_found",
            Self::Archived => "product_archived",
            Self::NotPriced => "product_not_priced",
            Self::CategoryNotFound => "category_not_found",
            Self::CategoryArchived => "category_archived",
            Self::CategoryCycle => "category_cycle",
            Self::CategoryTooDeep => "category_too_deep",
            Self::TooManyCategories => "too_many_categories",
            Self::TooManyImages => "too_many_images",
            Self::InvalidImage => "unsupported_image",
            Self::InvalidAlt => "invalid_alt",
            Self::ImageNotFound => "image_not_found",
            Self::InvalidImageOrder => "invalid_image_order",
            Self::TooManyOptions => "too_many_options",
            Self::InvalidOptionName => "invalid_option_name",
            Self::OptionExists => "option_exists",
            Self::InvalidOptionValues => "invalid_option_values",
            Self::OptionNotFound => "option_not_found",
            Self::OptionInUse => "option_in_use",
            Self::OptionValueInUse => "option_value_in_use",
            Self::TooManyVariants => "too_many_variants",
            Self::InvalidSelection => "invalid_selection",
            Self::VariantExists => "variant_exists",
            Self::VariantNotFound => "variant_not_found",
            Self::CurrencyMismatch => "currency_mismatch",
            Self::InvalidSku => "invalid_sku",
            Self::SkuTaken => "sku_taken",
            Self::InvalidAttribute => "invalid_attribute",
            Self::TooManyAttributes => "too_many_attributes",
            Self::InvalidTaxCategory => "invalid_tax_category",
            Self::InvalidStock => "invalid_stock",
            Self::OutOfStock { .. } => "out_of_stock",
            Self::Write(err) => return timada_core::write_refusal(err),
            Self::Other(_) => return None,
        })
    }
}
