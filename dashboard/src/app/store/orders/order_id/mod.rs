//! `GET /store/orders/{order_id}`: the order (an `OrderState`), or 404
//! `{"error": "order_not_found"}` for an id the tenant has no order under,
//! another tenant's included. `documents` answers its invoice and credit
//! notes, `parcels` where its parcels are; `cancel` is the shopper
//! cancelling it.

pub mod cancel;
pub mod documents;
pub mod parcels;

use topcoat::{
    Result,
    context::{Cx, context},
    router::{module_param, route},
};

use super::super::carts::{Answer, deliver, gateway};
use crate::cluster::LocalReads;
use crate::store_carts;

module_param!(pub order_id);

#[route(GET)]
pub async fn index(cx: &Cx) -> Result<Answer> {
    let orders = match gateway(cx).await? {
        Ok(orders) => orders,
        Err(answer) => return Ok(answer),
    };
    deliver(store_carts::order(&orders, context::<LocalReads>(cx), order_id(cx)).await)
}
