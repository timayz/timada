//! `GET /store/carts/{cart_id}/order`: the order the cart became once its
//! payment went through (an `OrderState`), or 404 while there is none.

use topcoat::{
    Result,
    context::Cx,
    router::{path_param, route},
};

use super::super::{Answer, deliver, gateway};
use super::CartId;
use crate::store_carts;

#[route(GET)]
pub async fn order(cx: &Cx) -> Result<Answer> {
    let orders = match gateway(cx).await? {
        Ok(orders) => orders,
        Err(answer) => return Ok(answer),
    };
    deliver(store_carts::order_of_cart(&orders, path_param::<CartId>(cx)).await)
}
