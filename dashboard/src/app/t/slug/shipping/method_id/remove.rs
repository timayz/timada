use topcoat::{Result, context::Cx, router::error::SeeOther, router::route};

use super::act;

#[route(POST)]
pub async fn remove(cx: &Cx) -> Result<SeeOther> {
    act(cx, "Shipping method removed.", async |orders, actor, id| {
        orders.remove_shipping_method(actor, id).await
    })
    .await
}
