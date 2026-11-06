//! `POST /store/orders/{order_id}/cancel`: the shopper cancels their order,
//! while the shop has not bought its goods from the supplier yet. The
//! storefront vouches that the shopper holds the order, as for the order
//! itself. The hold goes back with Stripe at once; 200 with the order (an
//! `OrderState`): cancelled, nothing charged, or still cancelling while
//! Stripe did not answer (the payments sweep settles it); 404 for an id the
//! tenant has no order under; 422 `order_in_preparation` once it is too late.

use std::sync::Arc;

use serde_json::json;
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{content::Json, path_param, route},
};

use super::super::super::carts::{Answer, deliver};
use super::super::super::{store, storefront};
use super::OrderId;
use crate::payments::Payments;
use crate::payments::settle::settle_cancellation;
use crate::store_carts;

#[route(POST)]
pub async fn cancel(cx: &Cx) -> Result<Answer> {
    let access = match storefront(cx).await? {
        Ok(access) => access,
        Err(code) => return Ok((code, Json(json!({})))),
    };
    let store = store(cx, &access);
    let orders = store_carts::gateway(store.executor().clone(), store.tenant_id());
    let order_id = path_param::<OrderId>(cx);
    if let Err(err) = orders.request_shopper_cancellation(order_id).await {
        return deliver(store_carts::refused(err));
    }
    let payments = app_context::<Arc<Payments>>(cx);
    if let Some(account) = payments
        .account_of(&access.tenant)
        .map_err(topcoat::Error::from_anyhow)?
    {
        settle_cancellation(payments, &account, &orders, order_id)
            .await
            .map_err(topcoat::Error::from_anyhow)?;
    }
    deliver(store_carts::order(&orders, order_id).await)
}
