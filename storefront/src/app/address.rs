//! Where the shop's categories and products are read: a category at the
//! root, by its path in the tree (`/jeux-pour-chat/souris`), a product
//! under the path of a category it is filed under
//! (`/jeux-pour-chat/souris/souris-en-peluche`).
//!
//! The router knows neither: a path of any depth has no route. The pages
//! stay routed at `/c/{slug}` and `/p/{slug}`, and [`Addresses`] hands them
//! a path no fixed route answered, once the library says what it names
//! ([`resolve_path`]). Asked for directly, the two routes move to the
//! path, for good; a product filed under no category has none and keeps
//! `/p/{slug}`.
//!
//! A listing links a product under the category whose page it is
//! ([`product_href_under`]); everything else links it under its main one
//! ([`product_href`]), which is also the address search engines are told.

use timada_storefront::{Addressed, Category, CategoryNode, Paths, Product, resolve_path};
use topcoat::{
    Result,
    context::{Cx, memoize, try_context},
    router::{
        Body, Layer, LayerFuture, Method, Next, Path,
        error::{NotFoundError, redirect_permanent, rewrite},
        href,
        request::{method, uri},
    },
};

use crate::app::{c, p};

/// The paths of the shop's categories, in the language of the request,
/// read once per request.
#[memoize(as_ref)]
pub async fn paths(cx: &Cx) -> Result<Paths> {
    timada_storefront::paths(cx).await
}

/// Where a category's page is: its path, or its own route for one the
/// tree does not hold.
pub fn category_href(cx: &Cx, paths: &Paths, category: &Category) -> String {
    match paths.category(&category.id) {
        Some(path) => path.to_owned(),
        None => href!(c::slug::index, c::slug::slug: category.slug.clone()).resolve(cx),
    }
}

/// A product's default address: under its main category, or at its own
/// route when it is filed under none.
pub fn product_href(cx: &Cx, paths: &Paths, product: &Product) -> String {
    filed_href(cx, paths, &product.categories, &product.slug)
}

/// [`product_href`] for what only knows the product's slug and the
/// categories it is filed under: a line of the cart.
pub fn filed_href(cx: &Cx, paths: &Paths, categories: &[String], slug: &str) -> String {
    match paths.filed(categories, slug) {
        Some(path) => path,
        None => href!(p::slug::index, p::slug::slug: slug.to_owned()).resolve(cx),
    }
}

/// A product's address under the category `category_id`, whose page lists
/// it: its own when the product is filed under it, the default one when it
/// is only filed under a category below.
pub fn product_href_under(cx: &Cx, paths: &Paths, category_id: &str, product: &Product) -> String {
    if product.categories.iter().any(|id| id == category_id)
        && let Some(path) = paths.under(category_id, &product.slug)
    {
        return path;
    }
    product_href(cx, paths, product)
}

/// `path` with `query` written after it; nothing more when the query says
/// nothing.
pub fn with_query(path: &str, query: &impl serde::Serialize) -> String {
    match serde_urlencoded::to_string(query) {
        Ok(query) if !query.is_empty() => format!("{path}?{query}"),
        _ => path.to_owned(),
    }
}

/// The category `category_id` and the ones above it, top-level first: the
/// breadcrumb of what is read under it.
pub fn trail(tree: &[CategoryNode], category_id: &str) -> Vec<Category> {
    fn walk(nodes: &[CategoryNode], id: &str, above: &mut Vec<Category>) -> bool {
        for node in nodes {
            above.push(node.category.clone());
            if node.category.id == id || walk(&node.children, id, above) {
                return true;
            }
            above.pop();
        }
        false
    }
    let mut above = Vec::new();
    walk(tree, category_id, &mut above);
    above
}

/// What [`Addresses`] hands a page it reached by a path: the category the
/// product is read under. A request without one asked for the page's own
/// route.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Via {
    /// The category of the path a product was asked under; `None` for a
    /// category's page.
    pub category_id: Option<String>,
}

/// The path the page of this request was reached by, if it was.
pub fn via(cx: &Cx) -> Option<&Via> {
    try_context::<Via>(cx)
}

/// The request's query string, with its `?`, or nothing.
pub fn query_of(cx: &Cx) -> String {
    uri(cx)
        .query()
        .map(|query| format!("?{query}"))
        .unwrap_or_default()
}

/// Wraps every request; register it on the router with no path, inside
/// the layer that answers a moved address.
pub struct Addresses;

impl Layer for Addresses {
    fn path(&self) -> Option<&Path> {
        None
    }

    fn handle<'a>(&'a self, cx: &'a Cx, body: Body, next: Next<'a>) -> LayerFuture<'a> {
        Box::pin(async move {
            let result = next.run(cx, body).await;
            let missing = matches!(&result, Err(err) if err.is::<NotFoundError>());
            // Only an address nothing answered, and once: a page reached
            // by its path that then finds nothing is a 404.
            if !missing || via(cx).is_some() || !matches!(*method(cx), Method::GET | Method::HEAD) {
                return result;
            }
            let named = async {
                let paths = paths(cx).await?;
                resolve_path(cx, paths, uri(cx).path()).await
            };
            let query = query_of(cx);
            match named.await {
                Ok(Some(Addressed::Category(slug))) => {
                    let page = href!(c::slug::index, c::slug::slug: slug).resolve(cx);
                    Err(rewrite(format!("{page}{query}"), Body::empty())
                        .with(Via { category_id: None })
                        .into())
                }
                Ok(Some(Addressed::Product { slug, category_id })) => {
                    let page = href!(p::slug::index, p::slug::slug: slug).resolve(cx);
                    Err(rewrite(format!("{page}{query}"), Body::empty())
                        .with(Via {
                            category_id: Some(category_id),
                        })
                        .into())
                }
                Ok(Some(Addressed::Moved(path))) => {
                    Err(redirect_permanent(format!("{path}{query}")).into())
                }
                Ok(Some(Addressed::Unfiled(slug))) => {
                    let page = href!(p::slug::index, p::slug::slug: slug).resolve(cx);
                    Err(redirect_permanent(format!("{page}{query}")).into())
                }
                Ok(None) => result,
                Err(err) => {
                    // The 404 stands: a lookup that failed is no reason to
                    // turn it into a 500.
                    tracing::warn!(error = %err, "cannot look up what a path names");
                    result
                }
            }
        })
    }
}
