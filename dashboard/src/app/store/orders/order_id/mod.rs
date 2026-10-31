//! `GET /store/orders/{order_id}`: the order (an `OrderState`), or 404
//! `{"error": "order_not_found"}` for an id the tenant has no order under,
//! another tenant's included.

use topcoat::{
    Result,
    context::Cx,
    router::{module_param, path_param, route},
};

use super::super::carts::{Answer, deliver, gateway};
use crate::store_carts;

module_param!(pub order_id);

#[route(GET)]
pub async fn index(cx: &Cx) -> Result<Answer> {
    let orders = match gateway(cx).await? {
        Ok(orders) => orders,
        Err(answer) => return Ok(answer),
    };
    deliver(store_carts::order(&orders, path_param::<OrderId>(cx)).await)
}
