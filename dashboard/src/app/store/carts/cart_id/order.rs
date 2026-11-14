//! `GET /store/carts/{cart_id}/order`: the order the cart became once its
//! payment went through (an `OrderState`), or 404 while there is none.

use topcoat::{
    Result,
    context::{Cx, context},
    router::route,
};

use super::super::{Answer, deliver, gateway};
use super::cart_id;
use crate::cluster::LocalReads;
use crate::store_carts;

#[route(GET)]
pub async fn order(cx: &Cx) -> Result<Answer> {
    let orders = match gateway(cx).await? {
        Ok(orders) => orders,
        Err(answer) => return Ok(answer),
    };
    deliver(store_carts::order_of_cart(&orders, context::<LocalReads>(cx), cart_id(cx)).await)
}
