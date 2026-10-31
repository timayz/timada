//! `POST /store/carts/{cart_id}/quantity` with a `CartItem`: sets how many
//! of that line the cart holds; zero removes it. 200 `{}`, or the refusal.

use bytes::Bytes;
use timada_orders::CartItem;
use topcoat::{
    Result,
    context::Cx,
    router::{path_param, route},
};

use super::super::{Answer, deliver, gateway, parse_body};
use super::CartId;
use crate::store_carts;

#[route(POST)]
pub async fn quantity(cx: &Cx, bytes: Bytes) -> Result<Answer> {
    let orders = match gateway(cx).await? {
        Ok(orders) => orders,
        Err(answer) => return Ok(answer),
    };
    let item: CartItem = match parse_body(&bytes) {
        Ok(item) => item,
        Err(answer) => return Ok(answer),
    };
    deliver(store_carts::set_quantity(&orders, path_param::<CartId>(cx), item).await)
}
