//! What a page asks for: the store, a product, a page of products. Errors
//! are topcoat errors, so `?` works in a page.

use std::borrow::Cow;

use topcoat_core::context::{Cx, context, try_context};
use topcoat_core::error::{Error, Result};
use topcoat_router::error::not_found;

use crate::money::MoneyFormat;
use crate::query::{
    Category, CategoryHub, CategoryNode, FacetValue, Product, ProductPage, ProductQuery, Seo,
    category_ancestors, menu_of, published_categories, published_category_by_slug,
    published_category_hub, published_listing_filters, published_option_values,
    published_product_by_slug, published_products,
};
use crate::store::Store;

/// The store the host registered with
/// [`RouterBuilderStoreExt::store`](crate::RouterBuilderStoreExt::store).
///
/// # Panics
///
/// Panics when none was registered.
pub fn store(cx: &Cx) -> &Store {
    context::<Store>(cx)
}

/// Where this store's product pictures are loaded from, when the host set
/// [`StoreConfig::media_url`](crate::StoreConfig::media_url).
pub fn media_url(cx: &Cx) -> Option<&str> {
    store(cx).media_url()
}

/// How an amount of money is written for this request: the
/// [`MoneyFormat`] the host carries in the request's context, when it
/// writes prices one way per language (`12,50€` in French, `€12.50` in
/// English), else the store's
/// ([`StoreConfig::money`](crate::StoreConfig::money)). For a page that
/// formats cents of its own; the views the library's readers return for a
/// request already carry it.
pub fn money(cx: &Cx) -> &MoneyFormat {
    try_context::<MoneyFormat>(cx).unwrap_or_else(|| store(cx).money())
}

/// A page of the published catalog.
pub async fn products(cx: &Cx, query: &ProductQuery) -> Result<ProductPage> {
    let store = store(cx);
    let query = &searched_in(cx, query);
    let mut page = published_products(store.db(), query)
        .await
        .map_err(Error::from_anyhow)?;
    for product in &mut page.items {
        product.fill_urls(store.media_url());
    }
    in_language(cx, &mut page.items).await?;
    Ok(page)
}

/// `query`, its search read in the language of the request when that is
/// not the main one: what a visitor types is matched against what they
/// read.
fn searched_in<'a>(cx: &Cx, query: &'a ProductQuery) -> Cow<'a, ProductQuery> {
    match crate::read_in(cx) {
        Some(language) if query.q.is_some() && query.language.is_none() => {
            Cow::Owned(query.clone().read_in(language))
        }
        _ => Cow::Borrowed(query),
    }
}

/// Words `products` in the language the request is read in, when it is not
/// the main one.
async fn in_language(cx: &Cx, products: &mut [Product]) -> Result<()> {
    match crate::read_in(cx) {
        Some(language) => crate::read_in::products_in(store(cx).db(), language, products)
            .await
            .map_err(Error::from_anyhow),
        None => Ok(()),
    }
}

/// The values `option` takes across the products `query` lists, each with
/// its count, for a filter's links: see
/// [`published_option_values`](crate::published_option_values).
///
/// A request read in another language ([`read_in`](crate::read_in)) gets
/// each value labelled as the shop words it there; its key, what a link
/// carries, is the main language's either way.
pub async fn option_values(cx: &Cx, query: &ProductQuery, option: &str) -> Result<Vec<FacetValue>> {
    let db = store(cx).db();
    let mut values = published_option_values(db, &searched_in(cx, query), option)
        .await
        .map_err(Error::from_anyhow)?;
    if let Some(language) = crate::read_in(cx) {
        let words = crate::published_words(
            db,
            language,
            values.iter().map(|value| value.label.as_str()),
        )
        .await
        .map_err(Error::from_anyhow)?;
        for value in &mut values {
            if let Some(wording) = words.get(&timada_catalog::word_key(&value.label)) {
                value.label = wording.clone();
            }
        }
    }
    Ok(values)
}

/// A published product by slug, if any.
///
/// A request read in another language ([`read_in`](crate::read_in)) finds
/// it by its slug in that language, and by its main slug when it has no
/// translation there; the product comes back worded in the language, under
/// the slug it goes by there, which a page compares with the one asked.
pub async fn product_by_slug(cx: &Cx, slug: &str) -> Result<Option<Product>> {
    let store = store(cx);
    let db = store.db();
    let mut product = None;
    if let Some(language) = crate::read_in(cx)
        && let Some(id) =
            crate::translated_slug(db, crate::TranslationKind::Product, language, slug)
                .await
                .map_err(Error::from_anyhow)?
    {
        product = crate::query::published_products_by_ids(db, &[id.as_str()])
            .await
            .map_err(Error::from_anyhow)?
            .pop();
    }
    if product.is_none() {
        product = published_product_by_slug(db, slug)
            .await
            .map_err(Error::from_anyhow)?;
    }
    if let Some(product) = &mut product {
        product.fill_urls(store.media_url());
        in_language(cx, std::slice::from_mut(product)).await?;
    }
    Ok(product)
}

/// The slugs a product goes by, in the main language and in each it is
/// translated into: what a page names its other languages' addresses from.
pub async fn product_slugs(cx: &Cx, product_id: &str) -> Result<crate::Slugs> {
    Ok(
        crate::read_in::slugs_of(store(cx).db(), "product", product_id)
            .await
            .map_err(Error::from_anyhow)?
            .unwrap_or_default(),
    )
}

/// The same for a category.
pub async fn category_slugs(cx: &Cx, category_id: &str) -> Result<crate::Slugs> {
    Ok(
        crate::read_in::slugs_of(store(cx).db(), "category", category_id)
            .await
            .map_err(Error::from_anyhow)?
            .unwrap_or_default(),
    )
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
///
/// A request read in another language ([`read_in`](crate::read_in)) gets
/// them named in it, under the slugs they go by there.
pub async fn categories(cx: &Cx) -> Result<Vec<CategoryNode>> {
    let db = store(cx).db();
    let mut nodes = published_categories(db).await.map_err(Error::from_anyhow)?;
    if let Some(language) = crate::read_in(cx) {
        crate::read_in::categories_in(db, language, &mut nodes)
            .await
            .map_err(Error::from_anyhow)?;
    }
    Ok(nodes)
}

/// The trees a menu shows: [`categories`] without the ones the merchant hid
/// from the menu. A page that also needs the whole tree (to find its own
/// category, say) loads [`categories`] once and calls [`menu_of`] on it.
pub async fn menu(cx: &Cx) -> Result<Vec<CategoryNode>> {
    Ok(menu_of(&categories(cx).await?))
}

/// A live category by slug, if any.
///
/// A request read in another language finds it as
/// [`product_by_slug`] finds a product.
pub async fn category_by_slug(cx: &Cx, slug: &str) -> Result<Option<Category>> {
    let db = store(cx).db();
    let Some(language) = crate::read_in(cx) else {
        return published_category_by_slug(db, slug)
            .await
            .map_err(Error::from_anyhow);
    };
    let found = async {
        let mut category =
            match crate::translated_slug(db, crate::TranslationKind::Category, language, slug)
                .await?
            {
                Some(id) => crate::query::published_category_by_id(db, &id).await?,
                None => None,
            };
        if category.is_none() {
            category = published_category_by_slug(db, slug).await?;
        }
        if let Some(category) = &mut category {
            crate::read_in::category_in(db, language, category).await?;
        }
        anyhow::Ok(category)
    };
    found.await.map_err(Error::from_anyhow)
}

/// What a live category's page shows beside its listing: its buying guide,
/// the questions it answers and the products it puts first; see
/// [`published_category_hub`].
pub async fn category_hub(cx: &Cx, category_id: &str) -> Result<CategoryHub> {
    let store = store(cx);
    let mut hub = published_category_hub(store.db(), category_id, store.media_url())
        .await
        .map_err(Error::from_anyhow)?;
    for product in &mut hub.pinned {
        product.fill_urls(store.media_url());
    }
    if let Some(language) = crate::read_in(cx) {
        crate::read_in::hub_in(
            store.db(),
            language,
            category_id,
            store.media_url(),
            &mut hub,
        )
        .await
        .map_err(Error::from_anyhow)?;
    }
    Ok(hub)
}

/// The options the shop's listings are narrowed by, in the merchant's
/// order; `None` while the shop never set them. Each is an option's name:
/// [`option_values`] gives the values a listing offers for it, and
/// [`ProductQuery::with_option`] narrows by one. The names are the main
/// language's, which the values are looked up by: [`worded`](crate::worded)
/// gives what a visitor reading another language is shown for one.
pub async fn listing_filters(cx: &Cx) -> Result<Option<Vec<String>>> {
    published_listing_filters(store(cx).db())
        .await
        .map_err(Error::from_anyhow)
}

/// What search engines show for a category's page: the title and the
/// description the merchant wrote for them, else its name and description.
pub async fn category_seo(cx: &Cx, category: &Category) -> Result<Seo> {
    if let Some(language) = crate::read_in(cx)
        && let Some(seo) = crate::read_in::seo_in(
            store(cx).db(),
            language,
            crate::TranslationKind::Category,
            &category.id,
            &category.name,
            &category.description,
        )
        .await
        .map_err(Error::from_anyhow)?
    {
        return Ok(seo);
    }
    crate::query::published_category_seo(store(cx).db(), category)
        .await
        .map_err(Error::from_anyhow)
}

/// What search engines show for a product's page: the title and the
/// description the merchant wrote for them, else its name and the start of
/// its description.
pub async fn product_seo(cx: &Cx, product: &Product) -> Result<Seo> {
    if let Some(language) = crate::read_in(cx)
        && let Some(seo) = crate::read_in::seo_in(
            store(cx).db(),
            language,
            crate::TranslationKind::Product,
            &product.id,
            &product.name,
            &product.description,
        )
        .await
        .map_err(Error::from_anyhow)?
    {
        return Ok(seo);
    }
    crate::query::published_product_seo(store(cx).db(), product)
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
    let db = store(cx).db();
    let mut filed = crate::query::published_product_categories(db, &product.id)
        .await
        .map_err(Error::from_anyhow)?;
    if let Some(language) = crate::read_in(cx) {
        for category in &mut filed {
            crate::read_in::category_in(db, language, category)
                .await
                .map_err(Error::from_anyhow)?;
        }
    }
    Ok(filed)
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
    in_language(cx, &mut related).await?;
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
    in_language(cx, &mut offered).await?;
    Ok(offered)
}
