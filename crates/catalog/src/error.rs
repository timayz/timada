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
    #[error("`{0}` is required")]
    Required(&'static str),
    #[error(transparent)]
    Write(#[from] evento::WriteError),
    #[error(transparent)]
    Server(#[from] anyhow::Error),
}
