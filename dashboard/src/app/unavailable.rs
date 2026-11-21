//! `/unavailable`: what is answered when this node cannot confirm a read.
//!
//! A read that must reflect every write before it is asked of a majority of
//! the cluster. A node cut off from the others, just back, or joining
//! cannot tell, and the store fails the read rather than answer from a file
//! that may be behind
//! ([`is_read_unconfirmed`](timada_core::is_read_unconfirmed)). For the ten
//! seconds or so before `/ready` turns the balancer away, such a node still
//! gets requests. Nothing is wrong with them, and asked again in a moment,
//! or of another node, they are answered: so the [`StoreAway`] layer
//! answers that failure, wherever a request met it, with a 503 to try
//! again after, where the router would answer its bare 500. It is the
//! read's counterpart of `writes_paused` and `write_not_taken`.
//!
//! A browser gets this page, which reads nothing from the store. Anything
//! else (a storefront on `/store`, a webhook, a script) gets the 503 alone:
//! a storefront takes it as the store being away (`store_unavailable`), and
//! a caller that retries does.
//!
//! The page is reached by an internal rewrite only: a browser asking for it
//! outright is sent home.

use topcoat::{
    Result,
    context::{Cx, try_context},
    router::{
        Body, HeaderMap, HeaderValue, Layer, LayerFuture, Method, Next, Path, StatusCode,
        error::{rewrite, see_other, service_unavailable},
        header::{ACCEPT, CACHE_CONTROL, CONTENT_LENGTH, CONTENT_TYPE, RETRY_AFTER},
        href, page,
        request::{headers, method, original_uri, uri},
    },
    view::{View, class, view},
};

use crate::components::button::{ButtonSize, ButtonVariant, button_variants};
use crate::components::card::CARD;
use crate::ui::auth_document;

/// Where the layer sends a browser whose request could not be answered.
const PATH: &str = "/unavailable";

/// How long a caller is told to wait before asking again: about what a node
/// just back takes to reach its peers, and less than the balancer takes to
/// turn away from one that stays cut off.
const RETRY_AFTER_SECS: u64 = 5;

/// Carried into the rewritten request: the page is answering for a read
/// that could not be confirmed. `retry` is the page that was asked for,
/// when coming back to it is a plain `GET`.
pub struct Unconfirmed {
    pub retry: Option<String>,
}

/// Wraps every request; register it on the router with no path, inside the
/// layer that logs failures. A request that failed on a read this node
/// could not confirm is answered 503: with [`index`](self::index) for a
/// browser, alone for anything else.
pub struct StoreAway;

impl Layer for StoreAway {
    fn path(&self) -> Option<&Path> {
        None
    }

    fn handle<'a>(&'a self, cx: &'a Cx, body: Body, next: Next<'a>) -> LayerFuture<'a> {
        Box::pin(async move {
            let result = next.run(cx, body).await;
            if try_context::<Unconfirmed>(cx).is_some() {
                // The rewritten dispatch: an answer about a moment, which
                // nothing should keep.
                let mut response = result?;
                *response.status_mut() = StatusCode::SERVICE_UNAVAILABLE;
                let headers = response.headers_mut();
                headers.insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
                headers.insert(RETRY_AFTER, HeaderValue::from(RETRY_AFTER_SECS));
                return Ok(response);
            }
            match result {
                Err(err) if is_unconfirmed(&err) => {
                    let method = method(cx);
                    tracing::warn!(
                        method = %method,
                        path = uri(cx).path(),
                        error = format!("{err:#}"),
                        "a read could not be confirmed: answering that the store is away"
                    );
                    if !asks_for_a_page(headers(cx)) {
                        return Err(service_unavailable(RETRY_AFTER_SECS).into());
                    }
                    let retry = (*method == Method::GET).then(|| {
                        original_uri(cx)
                            .path_and_query()
                            .map_or_else(|| "/".to_owned(), |path| path.as_str().to_owned())
                    });
                    // The handler consumed the body; the page reads none.
                    let mut without_body = headers(cx).clone();
                    without_body.remove(CONTENT_TYPE);
                    without_body.remove(CONTENT_LENGTH);
                    Err(rewrite(PATH, Body::empty())
                        .method(Method::GET)
                        .headers(without_body)
                        .with(Unconfirmed { retry })
                        .into())
                }
                other => other,
            }
        })
    }
}

/// Whether a request failed on a read this node could not confirm, behind
/// whatever the handler made of the store's error.
fn is_unconfirmed(err: &topcoat::Error) -> bool {
    err.chain().any(timada_core::is_read_unconfirmed)
}

/// Whether the caller is a browser loading a page, which shows what it is
/// answered: it says so in `Accept`. A storefront, a webhook or a script
/// asks for something else, or for anything.
fn asks_for_a_page(headers: &HeaderMap) -> bool {
    headers
        .get_all(ACCEPT)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .any(|range| {
            let kind = range.split(';').next().unwrap_or_default().trim();
            kind.eq_ignore_ascii_case("text/html")
        })
}

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let Some(unconfirmed) = try_context::<Unconfirmed>(cx) else {
        return Err(see_other(href!(crate::app::home).resolve(cx)).into());
    };
    Ok(view! {
        auth_document(
            title: "Unavailable for a moment",
            <div class=(class!(CARD, "gap-5 px-6 text-center")) data-store-away="">
                <h1 class="text-2xl font-bold tracking-tight">"Unavailable for a moment"</h1>
                <p role="alert" class="text-sm text-muted-foreground">
                    "This server cannot reach the others right now, so it could not answer. Nothing is wrong with what you asked."
                </p>
                if let Some(retry) = &unconfirmed.retry {
                    <p class="text-sm text-muted-foreground">"Try again in a few seconds."</p>
                    <a class=(button_variants(ButtonVariant::Primary, ButtonSize::Md)) href=(retry)>"Try again"</a>
                } else {
                    <p class="text-sm text-muted-foreground">
                        "Go back in a few seconds and check whether your change was saved before sending it again."
                    </p>
                }
            </div>
        )
    })
}

#[cfg(test)]
mod tests {
    use evento::cluster::ReadUnconfirmed;
    use timada_tenancy::TenancyError;

    use super::*;

    fn unconfirmed() -> anyhow::Error {
        anyhow::Error::new(ReadUnconfirmed::NoQuorum)
            .context("read barrier quorum not reached for Key(\"acme\")")
    }

    /// The store's failure is found as a handler hands it on: as it came,
    /// and behind a domain's error.
    #[test]
    fn a_read_left_unconfirmed_is_found_behind_what_a_handler_made_of_it() {
        assert!(is_unconfirmed(&topcoat::Error::from_anyhow(unconfirmed())));
        let through_a_domain: anyhow::Error = TenancyError::Other(unconfirmed()).into();
        assert!(is_unconfirmed(&topcoat::Error::from_anyhow(
            through_a_domain
        )));
        assert!(is_unconfirmed(
            &topcoat::Error::from_anyhow(unconfirmed()).context("loading the tenant")
        ));

        assert!(!is_unconfirmed(&topcoat::Error::from_anyhow(
            anyhow::anyhow!("database is locked")
        )));
        assert!(!is_unconfirmed(&topcoat::Error::from_anyhow(
            TenancyError::ApiKeyInvalid.into()
        )));
    }

    #[test]
    fn only_a_browser_loading_a_page_gets_the_page() {
        let accepting = |accept: &'static str| {
            let mut headers = HeaderMap::new();
            headers.insert(ACCEPT, HeaderValue::from_static(accept));
            headers
        };
        assert!(asks_for_a_page(&accepting(
            "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8"
        )));
        assert!(asks_for_a_page(&accepting("Text/HTML; q=0.9")));
        // A storefront, a script's `fetch`, a webhook.
        assert!(!asks_for_a_page(&accepting("application/json")));
        assert!(!asks_for_a_page(&accepting("*/*")));
        assert!(!asks_for_a_page(&HeaderMap::new()));
    }
}
