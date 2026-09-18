#[derive(Debug, thiserror::Error)]
pub enum CatalogError {
    #[error("product not found")]
    ProductNotFound,
    #[error("a product with sku `{0}` already exists")]
    SkuAlreadyExists(String),
    #[error("product is archived")]
    ProductArchived,
    #[error("`{0}` is required")]
    Required(&'static str),
    #[error(transparent)]
    Write(#[from] evento::WriteError),
    #[error(transparent)]
    Server(#[from] anyhow::Error),
}
