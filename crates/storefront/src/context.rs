//! What a page asks for: the store, a product, a page of products. Errors
//! are topcoat errors, so `?` works in a page.

use topcoat_core::context::{Cx, app_context};
use topcoat_core::error::{Error, Result};
use topcoat_router::error::not_found;

use crate::query::{
    Category, CategoryNode, Product, ProductPage, ProductQuery, category_ancestors,
    published_categories, published_category_by_slug, published_product_by_slug,
    published_products,
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

/// Where this store's product pictures are loaded from, when the host set
/// [`StoreConfig::media_url`](crate::StoreConfig::media_url).
pub fn media_url(cx: &Cx) -> Option<&str> {
    store(cx).media_url()
}

/// A page of the published catalog.
pub async fn products(cx: &Cx, query: &ProductQuery) -> Result<ProductPage> {
    let store = store(cx);
    let mut page = published_products(store.pool(), query)
        .await
        .map_err(|err| Error::from_anyhow(err.into()))?;
    for product in &mut page.items {
        product.fill_urls(store.media_url());
    }
    Ok(page)
}

/// A published product by slug, if any.
pub async fn product_by_slug(cx: &Cx, slug: &str) -> Result<Option<Product>> {
    let store = store(cx);
    let mut product = published_product_by_slug(store.pool(), slug)
        .await
        .map_err(|err| Error::from_anyhow(err.into()))?;
    if let Some(product) = &mut product {
        product.fill_urls(store.media_url());
    }
    Ok(product)
}

/// A published product by slug, or 404.
pub async fn require_product(cx: &Cx, slug: &str) -> Result<Product> {
    product_by_slug(cx, slug)
        .await?
        .ok_or_else(|| not_found().into())
}

/// A page of the published catalog in a category and everything under it.
pub async fn category_products(
    cx: &Cx,
    node: &CategoryNode,
    query: &ProductQuery,
) -> Result<ProductPage> {
    let query = query.clone().in_category(node.category.id.clone());
    products(cx, &query).await
}

/// The live categories as trees, each with how many published products are in
/// it and below it: what a navigation menu is built from.
pub async fn categories(cx: &Cx) -> Result<Vec<CategoryNode>> {
    published_categories(store(cx).pool())
        .await
        .map_err(|err| Error::from_anyhow(err.into()))
}

/// A live category by slug, if any.
pub async fn category_by_slug(cx: &Cx, slug: &str) -> Result<Option<Category>> {
    published_category_by_slug(store(cx).pool(), slug)
        .await
        .map_err(|err| Error::from_anyhow(err.into()))
}

/// A live category by slug, or 404.
pub async fn require_category(cx: &Cx, slug: &str) -> Result<Category> {
    category_by_slug(cx, slug)
        .await?
        .ok_or_else(|| not_found().into())
}

/// The live category of `slug` in a menu [`categories`] returned, or 404: a
/// page that shows the menu anyway finds its category without a query.
pub fn require_category_in<'a>(menu: &'a [CategoryNode], slug: &str) -> Result<&'a CategoryNode> {
    crate::query::find_category(menu, slug).ok_or_else(|| not_found().into())
}

/// The categories above this one, top-level first: a breadcrumb. `menu` is
/// what [`categories`] returned, so it costs no query.
pub fn category_path(menu: &[CategoryNode], category: &Category) -> Vec<Category> {
    category_ancestors(menu, category)
}

/// The live categories a product is filed under, in tree order: the first is
/// what a breadcrumb usually shows.
pub async fn product_categories(cx: &Cx, product: &Product) -> Result<Vec<Category>> {
    crate::query::published_product_categories(store(cx).pool(), &product.id)
        .await
        .map_err(|err| Error::from_anyhow(err.into()))
}

/// Published products sharing a category with this one, newest first, at most
/// `limit` of them and never the product itself.
pub async fn related_products(cx: &Cx, product: &Product, limit: u32) -> Result<Vec<Product>> {
    let store = store(cx);
    let mut related = crate::query::published_related_products(store.pool(), product, limit)
        .await
        .map_err(|err| Error::from_anyhow(err.into()))?;
    for product in &mut related {
        product.fill_urls(store.media_url());
    }
    Ok(related)
}
