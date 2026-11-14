//! `GET /store/carts/{cart_id}/requests/{request_id}`: a request to pay and
//! its answer so far (a `RequestAnswer`), or 404 for a cart or request the
//! tenant has not.

use topcoat::{
    Result,
    context::{Cx, context},
    router::{module_param, route},
};

use super::super::super::{Answer, deliver, gateway};
use super::super::cart_id;
use crate::cluster::LocalReads;
use crate::store_carts;

module_param!(pub request_id);

#[route(GET)]
pub async fn index(cx: &Cx) -> Result<Answer> {
    let orders = match gateway(cx).await? {
        Ok(orders) => orders,
        Err(answer) => return Ok(answer),
    };
    deliver(
        store_carts::request(
            &orders,
            context::<LocalReads>(cx),
            cart_id(cx),
            request_id(cx),
        )
        .await,
    )
}
