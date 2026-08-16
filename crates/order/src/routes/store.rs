//! Storefront checkout and order-status pages. Merged at the site root, so
//! these paths are public URLs.

use askama::Template;
use axum::extract::{Path, State};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::get;
use axum::{Form, Router};
use axum_extra::extract::cookie::CookieJar;
use timada_cart::{CartView, cart_cookie_id, load_cart};
use timada_core::{AppError, AppResult};
use timada_web::HtmlTemplate;

use crate::aggregate::Address;
use crate::commands::{PlaceOrderError, place_order};
use crate::state::OrderState;
use crate::view::{OrderView, load_order};

pub fn store_router(state: OrderState) -> Router {
    Router::new()
        .route("/checkout", get(checkout_page).post(submit_checkout))
        .route("/orders/{order_id}", get(order_page))
        .route("/orders/{order_id}/status", get(order_status_fragment))
        .with_state(state)
}

#[derive(Template)]
#[template(path = "store/checkout.html")]
struct CheckoutTemplate {
    cart: CartView,
}

#[derive(Template)]
#[template(path = "store/order.html")]
struct OrderTemplate {
    order: OrderView,
}

/// The `status` block of the order page, rendered on its own as the TwinSpark
/// reply to the refresh button. It is the whole `#order-status` div, so the
/// default `replace` swap keeps the id — and this endpoint — working for the
/// next refresh.
#[derive(Template)]
#[template(path = "store/order.html", block = "status")]
struct OrderStatusFragment {
    order: OrderView,
}

/// Nothing to check out without a usable cart, so send the customer back to the
/// cart page rather than showing an empty form.
async fn checkout_page(State(state): State<OrderState>, jar: CookieJar) -> AppResult<Response> {
    let Some(cart_id) = cart_cookie_id(&jar) else {
        return Ok(Redirect::to("/cart").into_response());
    };

    let cart = load_cart(&state.ctx.executor, &cart_id).await?;
    let Some(cart) = cart.filter(|cart| !cart.checked_out && !cart.is_empty()) else {
        return Ok(Redirect::to("/cart").into_response());
    };

    Ok(HtmlTemplate(CheckoutTemplate { cart }).into_response())
}

#[derive(serde::Deserialize)]
struct CheckoutForm {
    email: String,
    full_name: String,
    street: String,
    city: String,
    postal_code: String,
    country: String,
}

/// A plain form post, answered with a redirect so a reload cannot place the
/// order twice.
async fn submit_checkout(
    State(state): State<OrderState>,
    jar: CookieJar,
    Form(form): Form<CheckoutForm>,
) -> AppResult<Redirect> {
    let Some(cart_id) = cart_cookie_id(&jar) else {
        return Err(AppError::BadRequest(
            PlaceOrderError::UnknownCart.to_string(),
        ));
    };

    let address = Address {
        full_name: form.full_name,
        street: form.street,
        city: form.city,
        postal_code: form.postal_code,
        country: form.country,
    };

    // A refused checkout is the customer's mistake, not a server fault, so it
    // answers 400 with the reason instead of the blanket 500 that `?` on the
    // error type would produce.
    let order_id = match place_order(&state.ctx.executor, &cart_id, form.email, address).await {
        Ok(order_id) => order_id,
        Err(PlaceOrderError::Storage(error)) => return Err(AppError::Internal(error)),
        Err(refused) => return Err(AppError::BadRequest(refused.to_string())),
    };

    Ok(Redirect::to(&format!("/orders/{order_id}")))
}

async fn order_page(
    State(state): State<OrderState>,
    Path(order_id): Path<String>,
) -> AppResult<impl IntoResponse> {
    Ok(HtmlTemplate(OrderTemplate {
        order: current_order(&state, &order_id).await?,
    }))
}

/// TwinSpark endpoint: answers with the re-rendered status block only.
async fn order_status_fragment(
    State(state): State<OrderState>,
    Path(order_id): Path<String>,
) -> AppResult<impl IntoResponse> {
    Ok(HtmlTemplate(OrderStatusFragment {
        order: current_order(&state, &order_id).await?,
    }))
}

/// Replay the order rather than reading `admin_order_list`.
///
/// This is a read-your-own-write: the executor is `Rw` over one SQLite file, so
/// the redirect out of checkout lands on a page that already knows the order
/// exists. The SQL read model is fed by a subscription and would still be empty.
async fn current_order(state: &OrderState, order_id: &str) -> AppResult<OrderView> {
    load_order(&state.ctx.executor, order_id)
        .await?
        .ok_or(AppError::NotFound)
}
