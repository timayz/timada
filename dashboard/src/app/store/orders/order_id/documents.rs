//! `GET /store/orders/{order_id}/documents`: the invoice and credit notes of
//! the order so far (a `Vec<IssuedDocument>`), or 404 `{"error":
//! "order_not_found"}`. The storefront draws the PDF itself.

use topcoat::{
    Result,
    context::{Cx, context},
    router::route,
};

use super::super::super::carts::{Answer, deliver, gateway};
use super::order_id;
use crate::cluster::LocalReads;
use crate::store_carts;

#[route(GET)]
pub async fn documents(cx: &Cx) -> Result<Answer> {
    let orders = match gateway(cx).await? {
        Ok(orders) => orders,
        Err(answer) => return Ok(answer),
    };
    deliver(store_carts::documents(&orders, context::<LocalReads>(cx), order_id(cx)).await)
}
