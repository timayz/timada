//! `GET /store/orders/{order_id}/parcels`: where the order's parcels are, as
//! their carrier last said (a `Vec<ParcelState>`, empty until the first
//! news), or 404 `{"error": "order_not_found"}`.

use topcoat::{
    Result,
    context::{Cx, context},
    router::route,
};

use super::super::super::carts::{Answer, deliver, gateway};
use super::order_id;
use crate::cluster::LocalReads;
use crate::store_carts;

#[route(GET)]
pub async fn parcels(cx: &Cx) -> Result<Answer> {
    let orders = match gateway(cx).await? {
        Ok(orders) => orders,
        Err(answer) => return Ok(answer),
    };
    deliver(store_carts::parcels(&orders, context::<LocalReads>(cx), order_id(cx)).await)
}
