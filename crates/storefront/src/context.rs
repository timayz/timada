//! What a page asks for: the store, a product, a page of products. Errors
//! are topcoat errors, so `?` works in a page.

use topcoat_core::context::{Cx, app_context};
use topcoat_core::error::{Error, Result};
use topcoat_router::error::not_found;

use crate::query::{
    Product, ProductPage, ProductQuery, published_product_by_slug, published_products,
};
use crate::store::Store;

/// The store the host registered with
/// [`RouterBuilderStoreExt::store`](crate::RouterBuilderStoreExt::store).
///
/// # Panics
///
/// Panics when none was registered.
pub fn store(cx: &Cx) -> &Store {
    app_context::<Store>(cx)
}

/// A page of the published catalog.
pub async fn products(cx: &Cx, query: &ProductQuery) -> Result<ProductPage> {
    published_products(store(cx).pool(), query)
        .await
        .map_err(|err| Error::from_anyhow(err.into()))
}

/// A published product by slug, if any.
pub async fn product_by_slug(cx: &Cx, slug: &str) -> Result<Option<Product>> {
    published_product_by_slug(store(cx).pool(), slug)
        .await
        .map_err(|err| Error::from_anyhow(err.into()))
}

/// A published product by slug, or 404.
pub async fn require_product(cx: &Cx, slug: &str) -> Result<Product> {
    product_by_slug(cx, slug)
        .await?
        .ok_or_else(|| not_found().into())
}
