use topcoat::{
    Result,
    context::Cx,
    router::{error::SeeOther, route},
};

use super::act;

#[route(POST)]
pub async fn pay(cx: &Cx) -> Result<SeeOther> {
    act(cx, "Order marked as paid.", async |orders, actor, id| {
        orders.mark_order_paid(actor, id).await
    })
    .await
}
