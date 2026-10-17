//! What a shopper's forms post to: the library's own routes, served under
//! the store's prefix once the app gave a cookie key. Each reads its form,
//! acts on the store, keeps what the browser must remember in its cookies,
//! and sends the shopper back to one of the app's pages
//! ([`Paths`](crate::Paths)).
//!
//! Hand-built routes, never `#[route]`: the prefix is the app's to choose,
//! and a library's routes are registered by the host.

pub(crate) mod account;
pub(crate) mod cart;
pub(crate) mod checkout;
pub(crate) mod documents;
pub(crate) mod reviews;

use serde::de::DeserializeOwned;
use topcoat_core::context::Cx;
use topcoat_core::error::Result;
use topcoat_router::{Body, Method, Path, RouteFn, RouteFuture, RouterBuilder, parse_query_params};

use crate::storefront::Storefront;

pub use cart::{AddForm, DiscountForm, LineForm, OFFER_ENDED, OPTION_FIELD_PREFIX};
pub use checkout::{Asked, CheckoutForm, Started};

/// Turns `async fn handler(cx, input) -> Result<impl IntoResponse>` into
/// the function pointer a [`RouteFn`] takes: the body is read into `input`
/// first, and the outcome turned into a response.
macro_rules! action {
    ($name:ident = $handler:path, $input:ty) => {
        pub(crate) fn $name<'cx>(
            cx: &'cx ::topcoat_core::context::Cx,
            body: ::topcoat_router::Body,
        ) -> ::topcoat_router::RouteFuture<'cx> {
            Box::pin(async move {
                let input =
                    <$input as ::topcoat_router::request::FromRequest>::from_request(cx, body)
                        .await?;
                ::topcoat_router::response::IntoResponse::into_response(
                    $handler(cx, input).await?,
                    cx,
                )
            })
        }
    };
    ($name:ident = $handler:path) => {
        pub(crate) fn $name<'cx>(
            cx: &'cx ::topcoat_core::context::Cx,
            _body: ::topcoat_router::Body,
        ) -> ::topcoat_router::RouteFuture<'cx> {
            Box::pin(async move {
                ::topcoat_router::response::IntoResponse::into_response($handler(cx).await?, cx)
            })
        }
    };
}
pub(crate) use action;

/// The query string, or a 400.
pub(crate) fn query<T: DeserializeOwned>(cx: &Cx) -> Result<T> {
    parse_query_params(cx).map_err(|err| topcoat_router::error::bad_request(err.to_string()).into())
}

fn post(path: String, handle: for<'cx> fn(&'cx Cx, Body) -> RouteFuture<'cx>) -> RouteFn {
    RouteFn::new(Method::POST, Path::new(&path).to_owned(), handle)
}

fn get(path: String, handle: for<'cx> fn(&'cx Cx, Body) -> RouteFuture<'cx>) -> RouteFn {
    RouteFn::new(Method::GET, Path::new(&path).to_owned(), handle)
}

/// Registers every action under the store's prefix.
pub(crate) fn register(mut builder: RouterBuilder, storefront: &Storefront) -> RouterBuilder {
    let actions = &storefront.actions;
    builder = builder
        .route(post(actions.cart_add(), cart::add_route))
        .route(post(actions.cart_quantity(), cart::quantity_route))
        .route(post(actions.cart_remove(), cart::remove_route))
        .route(post(actions.cart_discount(), cart::discount_route))
        .route(post(actions.checkout_start(), checkout::start_route))
        .route(post(actions.checkout_place(), checkout::place_route))
        .route(post(
            actions.sign_in_request(),
            account::sign_in_request_route,
        ))
        .route(post(
            actions.sign_in_verify(),
            account::sign_in_verify_route,
        ))
        .route(post(actions.sign_out(), account::sign_out_route))
        .route(post(
            actions.sign_out_everywhere(),
            account::sign_out_everywhere_route,
        ))
        .route(post(actions.account_delete(), account::delete_route))
        .route(post(
            actions.address_remove(),
            account::address_remove_route,
        ))
        .route(post(actions.review_submit(), reviews::submit_route))
        .route(post(actions.review_revise(), reviews::revise_route))
        .route(get(actions.document_pdf_route(), documents::pdf_route));
    if let Some(pay) = &storefront.paths.checkout_pay {
        builder = builder.route(get(pay.to_string(), checkout::pay_route));
    }
    builder
}
