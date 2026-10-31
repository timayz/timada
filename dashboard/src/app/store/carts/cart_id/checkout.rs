//! `POST /store/carts/{cart_id}/checkout` with a `Checkout`: asks to pay for
//! the cart and waits (up to [`store_carts::PAYMENT_ANSWER_TIMEOUT`]) for
//! the payments subscription to open a payment or refuse. 200 with a
//! `RequestAnswer`, its `answer` `None` when nobody answered in time; or the
//! refusal of the request itself (`empty_cart`, `price_changed`, …).

use bytes::Bytes;
use timada_orders::Checkout;
use topcoat::{
    Result,
    context::Cx,
    router::{path_param, route},
};

use super::super::{Answer, deliver, gateway, parse_body};
use super::CartId;
use crate::store_carts;

#[route(POST)]
pub async fn checkout(cx: &Cx, bytes: Bytes) -> Result<Answer> {
    let orders = match gateway(cx).await? {
        Ok(orders) => orders,
        Err(answer) => return Ok(answer),
    };
    let checkout: Checkout = match parse_body(&bytes) {
        Ok(checkout) => checkout,
        Err(answer) => return Ok(answer),
    };
    deliver(store_carts::checkout(&orders, path_param::<CartId>(cx), checkout).await)
}
