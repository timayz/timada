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
//! (another tenant's included, and one dropped after lying idle), 422 with a
//! `Refusal` to show the shopper, 503 `writes_paused` or `write_not_taken`,
//! 400 for a body that is not what the call takes; 401 for a missing,
//! unknown or revoked key, 403 for a key that may not sell.
//!
//! Each key, and each visitor at a shop, may start only so many carts within
//! `[store]`'s window: past it, 429 `{"error": "too_many_carts"}` and nothing
//! is written. The visitor is the address the storefront reports in
//! `X-Shopper-IP`, else the caller's.

pub mod cart_id;

use std::sync::Arc;

use bytes::Bytes;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use timada_orders::{CartItem, Command, TOO_MANY_CARTS};
use timada_tenancy::ApiKeyAccess;
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{
        StatusCode,
        content::Json,
        request::{client_ip, headers},
        route,
    },
};

use crate::cluster::Executor;
use crate::config::Config;
use crate::rate_limit::{RateLimiter, allow_new_cart};
use crate::store_carts;

/// The header a storefront reports its visitor's address in.
const SHOPPER_IP: &str = "x-shopper-ip";

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
    Ok(Ok(orders_of(cx, &access)))
}

/// The orders gateway in the tenant of a key already checked.
fn orders_of(cx: &Cx, access: &ApiKeyAccess) -> Command<Executor> {
    let store = super::store(cx, access);
    store_carts::gateway(store.executor().clone(), store.tenant_id())
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
    let access = match super::storefront(cx).await? {
        Ok(access) => access,
        Err(code) => return Ok((code, Json(json!({})))),
    };
    let item: CartItem = match parse_body(&bytes) {
        Ok(item) => item,
        Err(answer) => return Ok(answer),
    };
    // Counted before the store is read for the item, so a visitor past the
    // limit costs it nothing, and a start refused for its item counts too
    // (the storefront offers no add for what is sold out).
    let allowed = allow_new_cart(
        app_context::<Arc<RateLimiter>>(cx),
        &app_context::<Config>(cx).store,
        &access.key.id,
        &access.tenant.id,
        headers(cx)
            .get(SHOPPER_IP)
            .and_then(|value| value.to_str().ok()),
        client_ip(cx),
    )
    .await;
    if !allowed {
        tracing::info!(
            tenant = %access.tenant.slug,
            key = %access.key.id,
            "cart refused: too many started"
        );
        return Ok((
            StatusCode::TOO_MANY_REQUESTS,
            Json(json!({ "error": TOO_MANY_CARTS })),
        ));
    }
    deliver(store_carts::start(&orders_of(cx, &access), item).await)
}
