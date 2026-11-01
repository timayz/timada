//! What a page asks for: the store, a product, a page of products. Errors
//! are topcoat errors, so `?` works in a page.

use topcoat_core::context::{Cx, app_context};
use topcoat_core::error::{Error, Result};
use topcoat_router::error::not_found;

use crate::money::MoneyFormat;
use crate::query::{
    Category, CategoryNode, FacetValue, Product, ProductPage, ProductQuery, category_ancestors,
    menu_of, published_categories, published_category_body, published_category_by_slug,
    published_option_values, published_product_by_slug, published_products,
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

/// How the store writes an amount of money
/// ([`StoreConfig::money`](crate::StoreConfig::money)), for a page that
/// formats cents of its own; the views the library builds already carry it.
pub fn money(cx: &Cx) -> &MoneyFormat {
    store(cx).money()
}

/// A page of the published catalog.
pub async fn products(cx: &Cx, query: &ProductQuery) -> Result<ProductPage> {
    let store = store(cx);
    let mut page = published_products(store.db(), query)
        .await
        .map_err(Error::from_anyhow)?;
    for product in &mut page.items {
        product.fill_urls(store.media_url());
    }
    Ok(page)
}

/// The values `option` takes across the products `query` lists, each with
/// its count, for a filter's links: see
/// [`published_option_values`](crate::published_option_values).
pub async fn option_values(cx: &Cx, query: &ProductQuery, option: &str) -> Result<Vec<FacetValue>> {
    published_option_values(store(cx).db(), query, option)
        .await
        .map_err(Error::from_anyhow)
}

/// A published product by slug, if any.
pub async fn product_by_slug(cx: &Cx, slug: &str) -> Result<Option<Product>> {
    let store = store(cx);
    let mut product = published_product_by_slug(store.db(), slug)
        .await
        .map_err(Error::from_anyhow)?;
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
    published_categories(store(cx).db())
        .await
        .map_err(Error::from_anyhow)
}

/// The trees a menu shows: [`categories`] without the ones the merchant hid
/// from the menu. A page that also needs the whole tree (to find its own
/// category, say) loads [`categories`] once and calls [`menu_of`] on it.
pub async fn menu(cx: &Cx) -> Result<Vec<CategoryNode>> {
    Ok(menu_of(&categories(cx).await?))
}

/// A live category by slug, if any.
pub async fn category_by_slug(cx: &Cx, slug: &str) -> Result<Option<Category>> {
    published_category_by_slug(store(cx).db(), slug)
        .await
        .map_err(Error::from_anyhow)
}

/// The text a live category's page shows below its products, empty when it
/// has none.
pub async fn category_body(cx: &Cx, category_id: &str) -> Result<String> {
    published_category_body(store(cx).db(), category_id)
        .await
        .map_err(Error::from_anyhow)
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
    crate::query::published_product_categories(store(cx).db(), &product.id)
        .await
        .map_err(Error::from_anyhow)
}

/// Published products sharing a category with this one, newest first, at most
/// `limit` of them and never the product itself.
pub async fn related_products(cx: &Cx, product: &Product, limit: u32) -> Result<Vec<Product>> {
    let store = store(cx);
    let mut related = crate::query::published_related_products(store.db(), product, limit)
        .await
        .map_err(Error::from_anyhow)?;
    for product in &mut related {
        product.fill_urls(store.media_url());
    }
    Ok(related)
}

/// Published products to offer beside a cart: sharing a category with what
/// it holds (the newest products when nothing does), newest first, at most
/// `limit` of them and never one already in it.
pub async fn cart_recommendations(
    cx: &Cx,
    cart: &crate::cart::CartView,
    limit: u32,
) -> Result<Vec<Product>> {
    let store = store(cx);
    let ids: Vec<String> = cart
        .lines
        .iter()
        .chain(&cart.unavailable)
        .map(|line| line.product_id.clone())
        .collect();
    let mut offered = crate::query::published_cart_recommendations(store.db(), &ids, limit)
        .await
        .map_err(Error::from_anyhow)?;
    for product in &mut offered {
        product.fill_urls(store.media_url());
    }
    Ok(offered)
}
