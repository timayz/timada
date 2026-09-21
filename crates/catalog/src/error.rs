#[derive(Debug, thiserror::Error)]
pub enum CatalogError {
    #[error("product not found")]
    ProductNotFound,
    #[error("a product with sku `{0}` already exists")]
    SkuAlreadyExists(String),
    #[error("product is archived")]
    ProductArchived,
    #[error("category not found")]
    CategoryNotFound,
    #[error("a category with slug `{0}` already exists")]
    SlugAlreadyExists(String),
    #[error("`{0}` is not a slug: lowercase letters, digits and hyphens")]
    InvalidSlug(String),
    #[error("category is archived")]
    CategoryArchived,
    #[error("a category cannot go under itself or one of its own subcategories")]
    CategoryCycle,
    #[error("categories nest {0} levels deep at most")]
    CategoryTooDeep(usize),
    #[error("a category is filtered by {0} specs at most")]
    TooManyFacets(usize),
    #[error("product family not found")]
    FamilyNotFound,
    #[error("a product family with slug `{0}` already exists")]
    FamilySlugAlreadyExists(String),
    #[error("product family is dissolved")]
    FamilyDissolved,
    #[error("a product family still holding variants cannot be dissolved")]
    FamilyNotEmpty,
    #[error("a product family is told apart by {0} options at most")]
    TooManyOptions(usize),
    #[error("an option takes {0} values at most")]
    TooManyOptionValues(usize),
    #[error("option `{0}` is named twice")]
    DuplicateOption(String),
    #[error("`{option}` = `{value}` is still the place of a variant")]
    OptionInUse { option: String, value: String },
    #[error("a variant needs a value for `{0}`")]
    MissingOptionValue(String),
    #[error("`{value}` is not a value of `{option}`")]
    UnknownOptionValue { option: String, value: String },
    #[error("another variant already stands there")]
    VariantPlaceTaken,
    #[error("the product is a variant of another family")]
    ProductInAnotherFamily,
    #[error("`{0}` is required")]
    Required(&'static str),
    #[error(transparent)]
    Write(#[from] evento::WriteError),
    #[error(transparent)]
    Server(#[from] anyhow::Error),
}
