//! Storefront cart pages. Merged at the site root, so these paths are public
//! URLs.

use askama::Template;
use axum::Form;
use axum::Router;
use axum::extract::{Path, State};
use axum::response::{IntoResponse, Redirect};
use axum::routing::{get, post};
use axum_extra::extract::cookie::CookieJar;
use timada_core::{AppError, AppResult, new_id};
use timada_web::HtmlTemplate;

use timada_core::Money;
use timada_promotion::{DiscountRefusal, discount_amount, discount_id, load_discount, validate};

use crate::commands::{AddItemError, add_item, apply_discount, remove_discount, remove_item};
use crate::cookie::{cart_cookie, cart_cookie_id};
use crate::state::CartState;
use crate::view::{CartView, load_cart};

pub fn store_router(state: CartState) -> Router {
    Router::new()
        .route("/cart", get(cart_page))
        .route("/cart/items", post(add_to_cart))
        .route("/cart/items/{product_id}/remove", post(remove_from_cart))
        .route("/cart/discount", post(apply_discount_code))
        .route("/cart/discount/remove", post(remove_discount_code))
        .with_state(state)
}

/// What the applied code is worth against the current cart — advisory: the
/// authoritative amount is re-derived at checkout.
struct CartDiscountInfo {
    code: String,
    amount: Money,
    total_after: Money,
}

#[derive(Template)]
#[template(path = "store/cart.html")]
struct CartTemplate {
    cart: CartView,
    discount: Option<CartDiscountInfo>,
    discount_error: Option<String>,
}

/// The `items` block of the cart page, rendered on its own as the TwinSpark
/// reply to a remove. It is the whole `#cart-items` div, so the default
/// `replace` swap keeps the id (and this endpoint) working for the next
/// remove.
#[derive(Template)]
#[template(path = "store/cart.html", block = "items")]
struct CartItemsFragment {
    cart: CartView,
    discount: Option<CartDiscountInfo>,
    discount_error: Option<String>,
}

async fn cart_page(State(state): State<CartState>, jar: CookieJar) -> AppResult<impl IntoResponse> {
    let cart = current_cart(&state, &jar).await?;
    let (discount, discount_error) = describe_discount(&state, &cart).await?;

    Ok(HtmlTemplate(CartTemplate {
        cart,
        discount,
        discount_error,
    }))
}

/// Resolve the cart's stored code into a display line, or the reason it no
/// longer works (it may have expired or been disabled while sitting here).
async fn describe_discount(
    state: &CartState,
    cart: &CartView,
) -> anyhow::Result<(Option<CartDiscountInfo>, Option<String>)> {
    let Some(code) = cart.discount_code.as_deref() else {
        return Ok((None, None));
    };

    match evaluate_code(state, code, cart).await? {
        Ok(amount) => {
            let total_after = cart
                .total()
                .subtract(amount)
                .unwrap_or_else(|_| cart.total());
            Ok((
                Some(CartDiscountInfo {
                    code: code.to_owned(),
                    amount,
                    total_after,
                }),
                None,
            ))
        }
        Err(refused) => Ok((None, Some(format!("{code}: {refused}")))),
    }
}

/// The advisory check: does the code exist, is it in its window, and what is
/// it worth against this cart? Usage-limit exhaustion is only caught at
/// checkout, where the counter can answer atomically.
async fn evaluate_code(
    state: &CartState,
    code: &str,
    cart: &CartView,
) -> anyhow::Result<Result<Money, DiscountRefusal>> {
    let Some(discount) = load_discount(&state.ctx.executor, &discount_id(code)).await? else {
        return Ok(Err(DiscountRefusal::UnknownCode));
    };
    if let Err(refused) = validate(&discount, timada_core::now_millis()) {
        return Ok(Err(refused));
    }
    Ok(discount_amount(discount.kind, cart.total()))
}

#[derive(serde::Deserialize)]
struct DiscountForm {
    code: String,
}

/// Attach a code. A code that fails the advisory check is not stored — the
/// page re-renders with the reason instead.
async fn apply_discount_code(
    State(state): State<CartState>,
    jar: CookieJar,
    Form(form): Form<DiscountForm>,
) -> AppResult<axum::response::Response> {
    let cart = current_cart(&state, &jar).await?;
    if cart.is_empty() {
        return Ok(Redirect::to("/cart").into_response());
    }

    let code = form.code.trim().to_uppercase();
    if let Err(refused) = evaluate_code(&state, &code, &cart).await? {
        let (discount, _) = describe_discount(&state, &cart).await?;
        return Ok(HtmlTemplate(CartTemplate {
            cart,
            discount,
            discount_error: Some(format!("{code}: {refused}")),
        })
        .into_response());
    }

    apply_discount(&state.ctx.executor, &cart.id, &code).await?;
    Ok(Redirect::to("/cart").into_response())
}

async fn remove_discount_code(
    State(state): State<CartState>,
    jar: CookieJar,
) -> AppResult<Redirect> {
    if let Some(cart_id) = cart_cookie_id(&jar) {
        remove_discount(&state.ctx.executor, &cart_id).await?;
    }
    Ok(Redirect::to("/cart"))
}

#[derive(serde::Deserialize)]
struct AddItemForm {
    product_id: String,
    /// Absent when a "Add to cart" button posts without a quantity input.
    #[serde(default = "one")]
    quantity: u32,
}

fn one() -> u32 {
    1
}

/// A plain form post, answered with a redirect so a reload does not re-add the
/// item. The jar is returned along with the redirect because this is where a
/// brand-new cart's cookie gets minted.
///
/// The price snapshotted onto the line is the one for the shopper's region's
/// currency; a store with no regions falls back to the product's base price.
async fn add_to_cart(
    State(state): State<CartState>,
    jar: CookieJar,
    Form(form): Form<AddItemForm>,
) -> AppResult<(CookieJar, Redirect)> {
    let product = timada_catalog::load_product(&state.ctx.executor, &form.product_id)
        .await?
        .ok_or(AppError::NotFound)?;

    let unit_price = match timada_region::current_region(&state.ctx.read_pool, &jar).await? {
        Some(region) => {
            let currency =
                timada_core::Currency::from_code(&region.currency).map_err(anyhow::Error::from)?;
            product.price_in(currency).ok_or_else(|| {
                AppError::BadRequest(format!(
                    "this product is not available in your region ({currency})"
                ))
            })?
        }
        None => product.base_price(),
    };

    let (jar, cart_id) = ensure_cart_id(&state, jar).await?;

    match add_item(
        &state.ctx.executor,
        &cart_id,
        &product,
        unit_price,
        form.quantity,
    )
    .await
    {
        Ok(()) => {}
        Err(AddItemError::Storage(error)) => return Err(AppError::Internal(error)),
        Err(refused) => return Err(AppError::BadRequest(refused.to_string())),
    }

    Ok((jar, Redirect::to("/cart")))
}

/// TwinSpark endpoint: answers with the re-rendered items fragment only.
///
/// Reading the cart back straight after the write is safe because the executor
/// is `Rw` over one SQLite file — the read pool sees the committed event
/// immediately, so this is a read-your-own-write, not an eventually-consistent
/// projection.
async fn remove_from_cart(
    State(state): State<CartState>,
    jar: CookieJar,
    Path(product_id): Path<String>,
) -> AppResult<impl IntoResponse> {
    if let Some(cart_id) = cart_cookie_id(&jar) {
        remove_item(&state.ctx.executor, &cart_id, &product_id).await?;
    }

    let cart = current_cart(&state, &jar).await?;
    let (discount, discount_error) = describe_discount(&state, &cart).await?;

    Ok(HtmlTemplate(CartItemsFragment {
        cart,
        discount,
        discount_error,
    }))
}

/// The cart this browser is shopping with, or an empty one.
///
/// A checked-out cart reads as empty: the cookie still names the cart that
/// became an order, and the customer is now shopping for the next one.
async fn current_cart(state: &CartState, jar: &CookieJar) -> anyhow::Result<CartView> {
    let Some(cart_id) = cart_cookie_id(jar) else {
        return Ok(CartView::default());
    };

    let cart = load_cart(&state.ctx.executor, &cart_id)
        .await?
        .filter(|cart| !cart.checked_out)
        .unwrap_or_default();

    Ok(cart)
}

/// Resolve the cart id to write to, minting a new one (and its cookie) when
/// the browser has none or carries a cart that has already been checked out.
async fn ensure_cart_id(state: &CartState, jar: CookieJar) -> anyhow::Result<(CookieJar, String)> {
    if let Some(cart_id) = cart_cookie_id(&jar) {
        let checked_out = load_cart(&state.ctx.executor, &cart_id)
            .await?
            .is_some_and(|cart| cart.checked_out);

        if !checked_out {
            return Ok((jar, cart_id));
        }
    }

    let cart_id = new_id();
    let jar = jar.add(cart_cookie(cart_id.clone()));

    Ok((jar, cart_id))
}
