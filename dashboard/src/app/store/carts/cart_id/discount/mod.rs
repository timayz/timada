//! `POST /store/carts/{cart_id}/discount` with `{"code"}`: puts the discount
//! code on the cart, replacing the one it had. 200 `{}`, or the refusal
//! (`discount_unknown` for a code the shop has not).

pub mod remove;

use bytes::Bytes;
use timada_orders::wire::DiscountCode;
use topcoat::{
    Result,
    context::Cx,
    router::{path_param, route},
};

use super::super::{Answer, deliver, gateway, parse_body};
use super::CartId;
use crate::store_carts;

#[route(POST)]
pub async fn apply(cx: &Cx, bytes: Bytes) -> Result<Answer> {
    let orders = match gateway(cx).await? {
        Ok(orders) => orders,
        Err(answer) => return Ok(answer),
    };
    let code: DiscountCode = match parse_body(&bytes) {
        Ok(code) => code,
        Err(answer) => return Ok(answer),
    };
    deliver(store_carts::apply_discount(&orders, path_param::<CartId>(cx), &code.code).await)
}
