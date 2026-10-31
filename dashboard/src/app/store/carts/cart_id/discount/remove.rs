//! `POST /store/carts/{cart_id}/discount/remove`: takes the discount code
//! off the cart. 200 `{}`, or the refusal.

use topcoat::{
    Result,
    context::Cx,
    router::{path_param, route},
};

use super::super::super::{Answer, deliver, gateway};
use super::super::CartId;
use crate::store_carts;

#[route(POST)]
pub async fn remove(cx: &Cx) -> Result<Answer> {
    let orders = match gateway(cx).await? {
        Ok(orders) => orders,
        Err(answer) => return Ok(answer),
    };
    deliver(store_carts::remove_discount(&orders, path_param::<CartId>(cx)).await)
}
