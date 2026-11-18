//! What search engines are told: `/robots.txt`, and `/sitemap.xml` with the
//! files it lists, `/sitemap-pages.xml` (the home, the catalogue, the
//! categories, the shop's own pages, the blog and its articles) and `/sitemap-products.xml?part=N`
//! (the products, [`SITEMAP_PRODUCTS`](timada_storefront::SITEMAP_PRODUCTS) a
//! file). All of it is read off the shop's own copy, so it answers while
//! the dashboard is away.

use timada_storefront::{
    SITEMAP_CONTENT_TYPE, Sitemap, SitemapIndex, absolute_url, robots_txt, sitemap_articles,
    sitemap_categories, sitemap_pages, sitemap_parts, sitemap_products,
};
use topcoat::{
    Result,
    context::Cx,
    router::{
        error::not_found,
        header::{CACHE_CONTROL, CONTENT_TYPE},
        href, query_params,
        response::IntoResponse,
        route,
    },
};

use crate::app::{address, blog, boutique, own_page, support_client};

/// What a shopper does rather than reads: nothing a search engine should
/// crawl or list. `robots.txt` keeps crawlers out of these and the layout
/// marks every page under them `noindex`.
pub const PRIVATE_PATHS: &[&str] = &[
    "/account",
    "/cart",
    "/checkout",
    "/cookies",
    "/emails",
    "/indisponible",
    "/newsletter",
    "/orders",
    "/reviews",
    "/search",
    "/store",
];

/// A listing searched, narrowed by a filter or sorted is the same products
/// again: crawlers stay out of it too. A page further down a listing
/// (`page=`) is not: it has products of its own.
const FILTERED: &[&str] = &["/*?*q=", "/*?*sort=", "/*?*min_price=", "/*?*max_price="];

/// How long a crawler may keep a file before asking again.
const CACHE: &str = "public, max-age=3600";

/// Whether `path` is one of the [`PRIVATE_PATHS`], or under one.
pub fn is_private(path: &str) -> bool {
    PRIVATE_PATHS.iter().any(|private| {
        path.strip_prefix(private)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
    })
}

#[topcoat::router::query_params(error = bad_request)]
struct ProductsQuery {
    part: Option<u32>,
}

#[derive(serde::Serialize)]
struct ProductsLink {
    part: u32,
}

fn xml(sitemap: String) -> impl IntoResponse {
    (
        [(CONTENT_TYPE, SITEMAP_CONTENT_TYPE), (CACHE_CONTROL, CACHE)],
        sitemap,
    )
}

#[route(GET "/sitemap.xml")]
pub async fn index(cx: &Cx) -> Result<impl IntoResponse> {
    // The files of each language, which list its own addresses.
    let speaking = crate::app::lang::speaking(cx).await;
    let mut index = SitemapIndex::new();
    for file in speaking.in_each(cx, &href!(pages).resolve(cx)) {
        index = index.sitemap(&file);
    }
    for part in 1..=sitemap_parts(cx).await? {
        let file = href!(products).query(ProductsLink { part }).resolve(cx);
        for file in speaking.in_each(cx, &file) {
            index = index.sitemap(&file);
        }
    }
    Ok(xml(index.xml()))
}

#[route(GET "/sitemap-pages.xml")]
pub async fn pages(cx: &Cx) -> Result<impl IntoResponse> {
    let speaking = crate::app::lang::speaking(cx).await;
    let mut sitemap = Sitemap::new()
        .url(
            &speaking.absolute(cx, &href!(crate::app::index).resolve(cx)),
            None,
        )
        .url(
            &speaking.absolute(cx, &href!(boutique::index).resolve(cx)),
            None,
        )
        .url(
            &speaking.absolute(cx, &href!(support_client::index).resolve(cx)),
            None,
        );
    let paths = address::paths(cx).await?;
    for category in sitemap_categories(cx).await? {
        // One the tree does not hold has no page.
        if let Some(path) = paths.category(&category.id) {
            sitemap = sitemap.url(&speaking.absolute(cx, path), Some(category.updated_at));
        }
    }
    // Only the pages the shop has an address for, and whose text is
    // published.
    for page in sitemap_pages(cx).await? {
        if let Some(path) = own_page::url(cx, &page.slug) {
            sitemap = sitemap.url(&speaking.absolute(cx, &path), Some(page.updated_at));
        }
    }
    // The blog once it has something to read, then each article.
    let articles = sitemap_articles(cx).await?;
    if let Some(latest) = articles.iter().map(|article| article.updated_at).max() {
        let path = href!(blog::index).resolve(cx);
        sitemap = sitemap.url(&speaking.absolute(cx, &path), Some(latest));
    }
    for article in articles {
        let path = href!(blog::slug::index, blog::slug::slug: article.slug.clone()).resolve(cx);
        sitemap = sitemap.url(&speaking.absolute(cx, &path), Some(article.updated_at));
    }
    Ok(xml(sitemap.xml()))
}

#[route(GET "/sitemap-products.xml")]
pub async fn products(cx: &Cx) -> Result<impl IntoResponse> {
    let part = query_params::<ProductsQuery>(cx)?.part.unwrap_or(1);
    let speaking = crate::app::lang::speaking(cx).await;
    let entries = sitemap_products(cx, part).await?;
    // A file past the last is not an empty sitemap: there is no such file.
    if entries.is_empty() && part != 1 {
        return Err(not_found().into());
    }
    let mut sitemap = Sitemap::new();
    // Each at its default address, the one its pages name as canonical.
    let paths = address::paths(cx).await?;
    for product in entries {
        let path = address::filed_href(cx, paths, &product.categories, &product.slug);
        sitemap = sitemap.url(&speaking.absolute(cx, &path), Some(product.updated_at));
    }
    Ok(xml(sitemap.xml()))
}

#[route(GET "/robots.txt")]
pub async fn robots(cx: &Cx) -> Result<impl IntoResponse> {
    // And out of each option the shop's listings are narrowed by.
    let options: Vec<String> = crate::app::filters::shop_filters(cx)
        .await?
        .iter()
        .map(|filter| format!("/*?*{}=", filter.key))
        .collect();
    let disallow: Vec<&str> = PRIVATE_PATHS
        .iter()
        .chain(FILTERED)
        .copied()
        .chain(options.iter().map(String::as_str))
        .collect();
    let sitemap = absolute_url(cx, &href!(index).resolve(cx));
    Ok((
        [
            (CONTENT_TYPE, "text/plain; charset=utf-8"),
            (CACHE_CONTROL, CACHE),
        ],
        robots_txt(&sitemap, &disallow),
    ))
}

#[cfg(test)]
mod tests {
    use super::is_private;

    #[test]
    fn a_private_path_covers_what_is_under_it_and_nothing_beside() {
        for path in ["/cart", "/cart/add", "/account/sign-in", "/orders/01J"] {
            assert!(is_private(path), "{path}");
        }
        for path in ["/", "/cartes", "/c/cart", "/p/account", "/boutique"] {
            assert!(!is_private(path), "{path}");
        }
    }
}
