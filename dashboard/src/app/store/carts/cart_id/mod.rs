//! `GET /store/carts/{cart_id}`: the cart priced as it stands, with the
//! shop's shipping methods (a `CartAnswer`), or 404.

pub mod checkout;
pub mod discount;
pub mod items;
pub mod order;
pub mod quantity;
pub mod requests;

use topcoat::{
    Result,
    context::Cx,
    router::{module_param, path_param, route},
};

use super::{Answer, deliver, gateway};
use crate::store_carts;

module_param!(pub cart_id);

#[route(GET)]
pub async fn index(cx: &Cx) -> Result<Answer> {
    let orders = match gateway(cx).await? {
        Ok(orders) => orders,
        Err(answer) => return Ok(answer),
    };
    deliver(store_carts::cart(&orders, path_param::<CartId>(cx)).await)
}
