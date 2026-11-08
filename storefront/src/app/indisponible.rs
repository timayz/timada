//! `/indisponible`: what a shopper sees when the dashboard is away.
//!
//! The catalogue is served from the shop's own copy, so a dashboard that is
//! down leaves browsing untouched. The cart, the checkout, the account, a
//! review, a message and the newsletter go through it and cannot work
//! meanwhile: the library fails such a call with
//! [`StoreUnavailable`](timada_storefront::StoreUnavailable), and the
//! [`UnavailablePage`] layer turns that failure, wherever a page met it,
//! into this page (a 503, not to be cached or indexed) instead of the
//! router's bare 500. The shell is the shop's, so the shopper keeps the
//! menu and the search, and the cart drawer says the cart is the part that
//! is away.
//!
//! The page is reached by an internal rewrite only: a browser asking for it
//! outright is sent home.

use timada_storefront::{STORE_UNAVAILABLE, is_store_unavailable, menu};
use topcoat::{
    Result,
    context::{Cx, try_request_context},
    router::{
        Body, HeaderValue, Layer, LayerFuture, Method, Next, Path, StatusCode,
        error::{rewrite, see_other},
        header::{CACHE_CONTROL, CONTENT_LENGTH, CONTENT_TYPE, RETRY_AFTER},
        href, page,
        request::{headers, method, original_uri, uri},
    },
    view::{View, view},
};

use crate::app::{document, say};

/// Where the layer sends a request the dashboard could not answer.
const PATH: &str = "/indisponible";

/// How long a browser or a proxy is told to wait before asking again.
const RETRY_AFTER_SECS: &str = "60";

/// Carried into the rewritten request: the page is answering an outage,
/// and the shell must not ask the dashboard again for the cart or the
/// shopper. `retry` is the page the shopper was reading, when coming back
/// to it is a plain `GET`.
pub struct StoreAway {
    pub retry: Option<String>,
}

/// Wraps every request; register it on the router with no path. A page that
/// failed because the dashboard is away is answered with
/// [`index`](self::index) and a 503.
pub struct UnavailablePage;

impl Layer for UnavailablePage {
    fn path(&self) -> Option<&Path> {
        None
    }

    fn handle<'a>(&'a self, cx: &'a Cx, body: Body, next: Next<'a>) -> LayerFuture<'a> {
        Box::pin(async move {
            let result = next.run(cx, body).await;
            if try_request_context::<StoreAway>(cx).is_some() {
                // The rewritten dispatch: an answer about an outage, which
                // nothing should keep or index.
                let mut response = result?;
                *response.status_mut() = StatusCode::SERVICE_UNAVAILABLE;
                let headers = response.headers_mut();
                headers.insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
                headers.insert(RETRY_AFTER, HeaderValue::from_static(RETRY_AFTER_SECS));
                return Ok(response);
            }
            match result {
                Err(err) if is_store_unavailable(&err) => {
                    let method = method(cx);
                    tracing::warn!(
                        method = %method,
                        path = uri(cx).path(),
                        error = %err,
                        "the dashboard is away: answering with the unavailable page"
                    );
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
                        .with(StoreAway { retry })
                        .into())
                }
                other => other,
            }
        })
    }
}

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let Some(away) = try_request_context::<StoreAway>(cx) else {
        return Err(see_other(href!(crate::app::index).resolve(cx)).into());
    };
    let nav = menu(cx).await?;
    Ok(view! {
        document(
            title: "Momentanément indisponible",
            nav: &nav,
            <div class="wrap unavailable" data-store-unavailable="">
                <h1>"Cette fonctionnalité est momentanément indisponible"</h1>
                <p class="notice" role="alert">(say(STORE_UNAVAILABLE))</p>
                <p>"Le panier, la commande, votre compte et nos formulaires ne répondent pas pour le moment. Rien n'est perdu : votre panier vous attend. En attendant, vous pouvez continuer à découvrir nos produits."</p>
                <p class="actions">
                    if let Some(retry) = &away.retry {
                        <a class="primary" href=(retry)>"Réessayer"</a>
                    }
                    <a href=(href!(crate::app::index))>"Retour à l'accueil"</a>
                </p>
            </div>
        )
    })
}
