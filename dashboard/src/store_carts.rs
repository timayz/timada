//! What the `/store/carts/*` and `/store/orders/*` endpoints answer a
//! storefront, without the HTTP: each call acts in the key's tenant through
//! the orders gateway and answers for the one cart or order whose id it was
//! given. The store API lets a storefront's key read neither the carts nor
//! the orders of its tenant ([`crate::read_only_store`]), so this is the only
//! way a storefront learns about a cart (its own, by the id it keeps in the
//! shopper's browser) or an order (the one its cart became, or one it
//! remembers), and the only way it fills a cart and asks to pay: a cart's
//! commands load the cart first, which its key may not.
//!
//! An id the tenant has no cart or order under is 404 `{"error":
//! "cart_not_found"}` (or `order_not_found`), another tenant's included: the
//! gateway is pinned to the key's tenant, so an id from elsewhere is simply
//! not there. A refusal to show the shopper is 422 with
//! [`timada_orders::wire::Refusal`]; the shop not taking writes for the
//! moment is 503 with `writes_paused`. Bodies and answers are JSON, the
//! shapes those of [`timada_orders::wire`]. The routes under
//! `crate::app::store::carts` and `crate::app::store::orders` do the HTTP;
//! the storefront library's `cart` module calls them.

use std::time::Duration;

use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use timada_orders::wire::{CartAnswer, Refusal, RequestAnswer, Started};
use timada_orders::{CartItem, Checkout, Command, OrderError, STOREFRONT_ACTOR};
use topcoat::router::StatusCode;

use crate::cluster::Executor;

/// How long a request to pay waits for the payments subscription's answer
/// before the storefront is told nobody answered.
pub const PAYMENT_ANSWER_TIMEOUT: Duration = Duration::from_secs(15);
/// How often it looks again when no write wakes it.
const PAYMENT_ANSWER_POLL: Duration = Duration::from_millis(250);

/// The status and JSON body to answer.
pub type Answer = (StatusCode, Value);

/// The orders gateway in the key's tenant.
pub fn gateway(executor: Executor, tenant_id: &str) -> Command<Executor> {
    Command::new(executor, tenant_id)
}

/// A JSON body as the type a call takes; `None` for one that is not.
pub fn parse<T: DeserializeOwned>(body: &[u8]) -> Option<T> {
    serde_json::from_slice(body).ok()
}

/// 400: the body is not what the call takes.
pub fn bad_request() -> Answer {
    (StatusCode::BAD_REQUEST, json!({ "error": "bad_request" }))
}

fn ok<T: serde::Serialize>(body: &T) -> anyhow::Result<Answer> {
    Ok((StatusCode::OK, serde_json::to_value(body)?))
}

fn not_found(code: &str) -> Answer {
    (StatusCode::NOT_FOUND, json!({ "error": code }))
}

/// The refusal as its status and body, or the failure itself for one that is
/// not the shopper's doing.
fn refused(err: OrderError) -> anyhow::Result<Answer> {
    match err {
        OrderError::CartNotFound => Ok(not_found("cart_not_found")),
        OrderError::OrderNotFound => Ok(not_found("order_not_found")),
        err => match Refusal::of(&err) {
            Some(refusal) if refusal.error == timada_core::WRITES_PAUSED => Ok((
                StatusCode::SERVICE_UNAVAILABLE,
                serde_json::to_value(refusal)?,
            )),
            Some(refusal) => Ok((
                StatusCode::UNPROCESSABLE_ENTITY,
                serde_json::to_value(refusal)?,
            )),
            None => Err(err.into()),
        },
    }
}

/// `GET /store/carts/{cart_id}`: the cart priced as it stands, with the
/// shop's shipping methods ([`CartAnswer`]), or 404.
pub async fn cart(orders: &Command<Executor>, cart_id: &str) -> anyhow::Result<Answer> {
    let Some(cart) = orders.load_cart(cart_id).await? else {
        return Ok(not_found("cart_not_found"));
    };
    let quote = match orders.quote_cart(cart_id).await {
        Ok(quote) => quote,
        Err(err) => return refused(err),
    };
    let shipping_methods = orders.shipping_methods().await?;
    ok(&CartAnswer {
        cart,
        quote,
        shipping_methods,
    })
}

/// `POST /store/carts` with a [`CartItem`]: starts a cart holding it and
/// answers 201 with its id ([`Started`]); a refusal of the item starts no
/// cart.
pub async fn start(orders: &Command<Executor>, item: CartItem) -> anyhow::Result<Answer> {
    let cart_id = match orders.create_cart(STOREFRONT_ACTOR).await {
        Ok(id) => id,
        Err(err) => return refused(err),
    };
    match orders.add_to_cart(STOREFRONT_ACTOR, &cart_id, item).await {
        Ok(_) => Ok((
            StatusCode::CREATED,
            serde_json::to_value(Started { id: cart_id })?,
        )),
        Err(err) => refused(err),
    }
}

/// `POST /store/carts/{cart_id}/items` with a [`CartItem`]: adds it.
pub async fn add_item(
    orders: &Command<Executor>,
    cart_id: &str,
    item: CartItem,
) -> anyhow::Result<Answer> {
    match orders.add_to_cart(STOREFRONT_ACTOR, cart_id, item).await {
        Ok(_) => ok(&json!({})),
        Err(err) => refused(err),
    }
}

/// `POST /store/carts/{cart_id}/quantity` with a [`CartItem`]: sets how many
/// of that line the cart holds; zero removes it.
pub async fn set_quantity(
    orders: &Command<Executor>,
    cart_id: &str,
    item: CartItem,
) -> anyhow::Result<Answer> {
    match orders
        .set_cart_quantity(STOREFRONT_ACTOR, cart_id, item)
        .await
    {
        Ok(_) => ok(&json!({})),
        Err(err) => refused(err),
    }
}

/// `POST /store/carts/{cart_id}/discount` with a code: puts it on the cart.
pub async fn apply_discount(
    orders: &Command<Executor>,
    cart_id: &str,
    code: &str,
) -> anyhow::Result<Answer> {
    match orders.apply_discount(STOREFRONT_ACTOR, cart_id, code).await {
        Ok(_) => ok(&json!({})),
        Err(err) => refused(err),
    }
}

/// `POST /store/carts/{cart_id}/discount/remove`: takes the code off.
pub async fn remove_discount(orders: &Command<Executor>, cart_id: &str) -> anyhow::Result<Answer> {
    match orders.remove_discount(STOREFRONT_ACTOR, cart_id).await {
        Ok(_) => ok(&json!({})),
        Err(err) => refused(err),
    }
}

/// `POST /store/carts/{cart_id}/checkout` with a [`Checkout`]: asks to pay
/// and waits up to [`PAYMENT_ANSWER_TIMEOUT`] for the payments subscription
/// to open a payment or refuse; answers the request and what it got
/// ([`RequestAnswer`], its `answer` `None` when nobody answered in time).
pub async fn checkout(
    orders: &Command<Executor>,
    cart_id: &str,
    checkout: Checkout,
) -> anyhow::Result<Answer> {
    let request_id = match orders
        .request_checkout(STOREFRONT_ACTOR, cart_id, checkout)
        .await
    {
        Ok(id) => id,
        Err(err) => return refused(err),
    };
    let mut watch = evento::Executor::write_watch(orders.executor());
    let deadline = tokio::time::Instant::now() + PAYMENT_ANSWER_TIMEOUT;
    loop {
        let cart = orders.load_cart(cart_id).await?;
        let answer = cart
            .as_ref()
            .and_then(|cart| cart.request(&request_id))
            .and_then(|request| request.answer.clone());
        if answer.is_some() || tokio::time::Instant::now() >= deadline {
            if answer.is_none() {
                tracing::warn!(cart_id, request_id, "no answer to a request to pay");
            }
            return ok(&RequestAnswer { request_id, answer });
        }
        // A write to the store wakes us; the poll covers a missed one.
        match watch.as_mut() {
            Some(watch) => {
                let _ = tokio::time::timeout(PAYMENT_ANSWER_POLL, watch.changed()).await;
            }
            None => tokio::time::sleep(PAYMENT_ANSWER_POLL).await,
        }
    }
}

/// `GET /store/carts/{cart_id}/requests/{request_id}`: a request to pay and
/// its answer so far, or 404 for a cart or request the tenant has not.
pub async fn request(
    orders: &Command<Executor>,
    cart_id: &str,
    request_id: &str,
) -> anyhow::Result<Answer> {
    let Some(cart) = orders.load_cart(cart_id).await? else {
        return Ok(not_found("cart_not_found"));
    };
    let Some(request) = cart.request(request_id) else {
        return Ok(not_found("payment_not_found"));
    };
    ok(&RequestAnswer {
        request_id: request.request_id.clone(),
        answer: request.answer.clone(),
    })
}

/// `GET /store/carts/{cart_id}/order`: the order the cart became, or 404
/// while it is none.
pub async fn order_of_cart(orders: &Command<Executor>, cart_id: &str) -> anyhow::Result<Answer> {
    match orders.load_order_of_cart(cart_id).await? {
        Some(order) => ok(&order),
        None => Ok(not_found("order_not_found")),
    }
}

/// `GET /store/orders/{order_id}`: the order, or 404.
pub async fn order(orders: &Command<Executor>, order_id: &str) -> anyhow::Result<Answer> {
    match orders.load_order(order_id).await? {
        Some(order) => ok(&order),
        None => Ok(not_found("order_not_found")),
    }
}
