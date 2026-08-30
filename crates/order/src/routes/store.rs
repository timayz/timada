//! Storefront checkout and order-status pages. Merged at the site root, so
//! these paths are public URLs.

use askama::Template;
use axum::extract::{Path, State};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::get;
use axum::{Form, Router};
use axum_extra::extract::cookie::CookieJar;
use timada_cart::{CartView, cart_cookie_id, load_cart};
use timada_core::{AppError, AppResult, Money};
use timada_tax::{TaxAssessmentRequest, TaxableLine};
use timada_web::HtmlTemplate;

use timada_customer::current_customer;

use crate::aggregate::Address;
use crate::commands::{PlaceOrderError, place_order};
use crate::projections::{AdminOrderRow, orders_for_customer};
use crate::state::OrderState;
use crate::view::{OrderView, load_order};

/// Enough history rows for an account page without paginating.
const HISTORY_LIMIT: i64 = 100;

pub fn store_router(state: OrderState) -> Router {
    Router::new()
        .route("/checkout", get(checkout_page).post(submit_checkout))
        .route("/orders/{order_id}", get(order_page))
        .route("/orders/{order_id}/status", get(order_status_fragment))
        .route("/account/orders", get(account_orders_page))
        .with_state(state)
}

/// The applied code as the summary shows it. Indicative like the tax line —
/// the authoritative redemption happens on submit.
struct CheckoutDiscount {
    code: String,
    amount: Money,
    total_after: Money,
}

#[derive(Template)]
#[template(path = "store/checkout.html")]
struct CheckoutTemplate {
    cart: CartView,
    /// Indicative: the country is not known until the form is submitted, so
    /// this is the calculator's default rate. The page says as much.
    total_tax: Money,
    discount: Option<CheckoutDiscount>,
    /// Prefilled from the signed-in customer; empty for guests.
    email: String,
}

#[derive(Template)]
#[template(path = "store/account_orders.html")]
struct AccountOrdersTemplate {
    orders: Vec<AdminOrderRow>,
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

    // The applied code, resolved the same advisory way the cart page does; a
    // code that stopped working shows up as a refusal on submit.
    let discount = match cart.discount_code.as_deref() {
        Some(code) => {
            match timada_promotion::load_discount(
                &state.ctx.executor,
                &timada_promotion::discount_id(code),
            )
            .await?
            {
                Some(view)
                    if timada_promotion::validate(&view, timada_core::now_millis()).is_ok() =>
                {
                    timada_promotion::discount_amount(view.kind, cart.total())
                        .ok()
                        .map(|amount| CheckoutDiscount {
                            code: view.code,
                            amount,
                            total_after: cart
                                .total()
                                .subtract(amount)
                                .unwrap_or_else(|_| cart.total()),
                        })
                }
                _ => None,
            }
        }
        None => None,
    };
    let per_line_discount = match &discount {
        Some(discount) => crate::commands::allocate_discount(&cart.lines, discount.amount),
        None => vec![0; cart.lines.len()],
    };

    // Prices are tax-inclusive, so this only breaks out how much of the total
    // is tax. The destination country arrives with the form, so the preview
    // uses the calculator's default rate and the page labels it as such.
    let assessment = state
        .tax
        .assess(TaxAssessmentRequest {
            country: String::new(),
            lines: cart
                .lines
                .iter()
                .zip(&per_line_discount)
                .map(|(line, discount_cents)| TaxableLine {
                    reference: line.product_id.clone(),
                    gross_unit_price: line.unit_price,
                    quantity: line.quantity,
                    discount: Money::new(*discount_cents, line.unit_price.currency),
                })
                .collect(),
        })
        .await
        .map_err(anyhow::Error::from)?;

    let email = current_customer(&state.customer, &jar)
        .await?
        .map(|customer| customer.email)
        .unwrap_or_default();

    Ok(HtmlTemplate(CheckoutTemplate {
        cart,
        total_tax: assessment.total_tax,
        discount,
        email,
    })
    .into_response())
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

    // A guest and a signed-in customer share the same form; the session is
    // the only difference, resolved here and snapshotted onto the order.
    let customer_id = current_customer(&state.customer, &jar)
        .await?
        .map(|customer| customer.id);

    // A refused checkout is the customer's mistake, not a server fault, so it
    // answers 400 with the reason instead of the blanket 500 that `?` on the
    // error type would produce.
    let order_id = match place_order(
        &state.ctx.executor,
        &state.tax,
        &state.ctx.write_pool,
        &cart_id,
        customer_id,
        form.email,
        address,
    )
    .await
    {
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

/// The signed-in customer's order history. Guests are sent to the login form.
async fn account_orders_page(
    State(state): State<OrderState>,
    jar: CookieJar,
) -> AppResult<Response> {
    let Some(customer) = current_customer(&state.customer, &jar).await? else {
        return Ok(Redirect::to("/login").into_response());
    };

    let orders = orders_for_customer(&state.ctx.read_pool, &customer.id, HISTORY_LIMIT).await?;

    Ok(HtmlTemplate(AccountOrdersTemplate { orders }).into_response())
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
