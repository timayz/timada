//! An address that moved answers with a permanent redirect instead of a
//! 404, so the links to it and what search engines know of it follow:
//! a path on the merchant's list of redirects, a product's, a page's or an
//! article's former slug, the one a translation left in its language, and
//! an archived product, sent to its category. A product or a category is
//! found by the last part of the address, whatever is before it, and sent
//! to where it is now.
//!
//! Only an address nothing answers is looked up: a slug something else
//! took since shows that thing.

use timada_storefront::{
    MovedProduct, moved_article, moved_category, moved_page, moved_product, product_by_slug,
    redirect_for,
};
use topcoat::{
    Result,
    context::Cx,
    router::{
        Body, Layer, LayerFuture, Method, Next, Path,
        error::{NotFoundError, redirect_permanent},
        href,
        request::method,
    },
};

use crate::app::{address, blog, own_page};

/// Wraps every request; register it on the router with no path.
pub struct MovedAddress;

impl Layer for MovedAddress {
    fn path(&self) -> Option<&Path> {
        None
    }

    fn handle<'a>(&'a self, cx: &'a Cx, body: Body, next: Next<'a>) -> LayerFuture<'a> {
        Box::pin(async move {
            let result = next.run(cx, body).await;
            let missing = matches!(&result, Err(err) if err.is::<NotFoundError>());
            if !missing || !matches!(*method(cx), Method::GET | Method::HEAD) {
                return result;
            }
            match moved_to(cx).await {
                Ok(Some(to)) => Err(redirect_permanent(to).into()),
                Ok(None) => result,
                Err(err) => {
                    // The 404 stands: a lookup that failed is no reason to
                    // turn it into a 500.
                    tracing::warn!(error = %err, "cannot look up where an address moved");
                    result
                }
            }
        })
    }
}

/// Where the address of the request went, if anywhere.
async fn moved_to(cx: &Cx) -> Result<Option<String>> {
    // The French path: a redirect, a former slug and a page's own address
    // are kept in it, and the answer is turned to the visitor's language
    // on its way out.
    let path = crate::app::lang::speaking(cx).await.internal;
    let path = path.as_str();
    if let Some(to) = redirect_for(cx, path).await? {
        return Ok(Some(to));
    }
    let segments: Vec<&str> = path.trim_matches('/').split('/').collect();
    match segments.as_slice() {
        ["blog", slug] => {
            return Ok(moved_article(cx, slug)
                .await?
                .map(|now| href!(blog::slug::index, blog::slug::slug: now).resolve(cx)));
        }
        // The shop's own pages are served at their slug.
        [slug] if !slug.is_empty() => {
            if let Some(now) = moved_page(cx, slug).await? {
                return Ok(own_page::url(cx, &now));
            }
        }
        _ => {}
    }
    // A product's address ends with its slug, a category's with its own,
    // wherever in the tree and at the routes they had before: the last
    // part is looked up among the slugs they left, and the answer is
    // where the thing is now.
    let Some(last) = segments.last().filter(|last| !last.is_empty()) else {
        return Ok(None);
    };
    let paths = address::paths(cx).await?;
    let category_at = |slug: &str| {
        paths
            .by_slug(slug)
            .and_then(|id| paths.category(id))
            .map(str::to_owned)
    };
    Ok(match moved_product(cx, last).await? {
        Some(MovedProduct::Product(now)) => product_by_slug(cx, &now)
            .await?
            .map(|product| address::product_href(cx, paths, &product)),
        Some(MovedProduct::Category(category)) => category_at(&category),
        // A category keeps its slug for life in the main language; its
        // translation's may move.
        None => match moved_category(cx, last).await? {
            Some(now) => category_at(&now),
            None => None,
        },
    })
}
