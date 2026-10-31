//! `/store/carts/*`: a storefront's cart, one at a time. The store API lets
//! its key read neither the carts nor the orders of the tenant
//! ([`crate::read_only_store`]), so the storefront names the cart it keeps
//! in the shopper's browser and the dashboard answers for that one, in the
//! key's tenant, and writes what the shopper does to it. The work is
//! [`crate::store_carts`]; this module is the HTTP: the key
//! (`Authorization: Bearer tk_…`, see [`super::storefront`]), JSON bodies
//! read as [`timada_orders::wire`] says, JSON answers.
//!
//! - `POST /store/carts` with a `CartItem`: starts a cart holding it, 201
//!   `{"id"}`.
//! - `GET /store/carts/{cart_id}`: the cart priced, a `CartAnswer`.
//! - `POST /store/carts/{cart_id}/items`, `…/quantity` with a `CartItem`.
//! - `POST /store/carts/{cart_id}/discount` with `{"code"}`,
//!   `…/discount/remove`.
//! - `POST /store/carts/{cart_id}/checkout` with a `Checkout`: asks to pay
//!   and waits for the answer, a `RequestAnswer`.
//! - `GET /store/carts/{cart_id}/requests/{request_id}`: a `RequestAnswer`.
//! - `GET /store/carts/{cart_id}/order`: the order the cart became.
//!
//! Answers: 404 `{"error": "cart_not_found"}` for a cart the tenant has not
//! (another tenant's included), 422 with a `Refusal` to show the shopper,
//! 503 `writes_paused`, 400 for a body that is not what the call takes; 401
//! for a missing, unknown or revoked key, 403 for a key that may not sell.

pub mod cart_id;

use bytes::Bytes;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use timada_orders::{CartItem, Command};
use topcoat::{
    Result,
    context::Cx,
    router::{StatusCode, content::Json, route},
};

use crate::cluster::Executor;
use crate::store_carts;

pub type Answer = (StatusCode, Json<Value>);

/// What [`store_carts`] answered, as the response.
pub fn deliver(answer: anyhow::Result<store_carts::Answer>) -> Result<Answer> {
    let (status, body) = answer.map_err(topcoat::Error::from_anyhow)?;
    Ok((status, Json(body)))
}

/// The orders gateway in the key's tenant, or the status the key gets.
pub async fn gateway(cx: &Cx) -> Result<std::result::Result<Command<Executor>, Answer>> {
    let access = match super::storefront(cx).await? {
        Ok(access) => access,
        Err(code) => return Ok(Err((code, Json(json!({}))))),
    };
    let store = super::store(cx, &access);
    Ok(Ok(store_carts::gateway(
        store.executor().clone(),
        store.tenant_id(),
    )))
}

/// The body as the type the call takes, or the 400 to answer.
pub fn parse_body<T: DeserializeOwned>(body: &Bytes) -> std::result::Result<T, Answer> {
    store_carts::parse(body).ok_or_else(|| {
        let (status, body) = store_carts::bad_request();
        (status, Json(body))
    })
}

#[route(POST)]
pub async fn start(cx: &Cx, bytes: Bytes) -> Result<Answer> {
    let orders = match gateway(cx).await? {
        Ok(orders) => orders,
        Err(answer) => return Ok(answer),
    };
    let item: CartItem = match parse_body(&bytes) {
        Ok(item) => item,
        Err(answer) => return Ok(answer),
    };
    deliver(store_carts::start(&orders, item).await)
}
