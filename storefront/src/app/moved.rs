//! An address that moved answers with a permanent redirect instead of a
//! 404, so the links to it and what search engines know of it follow:
//! a path on the merchant's list of redirects, a product's, a page's or an
//! article's former slug, and an archived product, sent to its category.
//!
//! Only an address nothing answers is looked up: a slug something else
//! took since shows that thing.

use timada_storefront::{MovedProduct, moved_article, moved_page, moved_product, redirect_for};
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

use crate::app::{blog, c, own_page, p};

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
    Ok(match segments.as_slice() {
        ["p", slug] => match moved_product(cx, slug).await? {
            Some(MovedProduct::Product(now)) => {
                Some(href!(p::slug::index, p::slug::slug: now).resolve(cx))
            }
            Some(MovedProduct::Category(category)) => {
                Some(href!(c::slug::index, c::slug::slug: category).resolve(cx))
            }
            None => None,
        },
        ["blog", slug] => moved_article(cx, slug)
            .await?
            .map(|now| href!(blog::slug::index, blog::slug::slug: now).resolve(cx)),
        // The shop's own pages are served at their slug.
        [slug] if !slug.is_empty() => match moved_page(cx, slug).await? {
            Some(now) => own_page::url(cx, &now),
            None => None,
        },
        _ => None,
    })
}
