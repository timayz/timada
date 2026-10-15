use topcoat::{Result, context::Cx, router::error::SeeOther, router::route};

use super::act;

#[route(POST)]
pub async fn reactivate(cx: &Cx) -> Result<SeeOther> {
    act(cx, "Discount reactivated.", async |discounts, actor, id| {
        discounts.reactivate_discount(actor, id).await
    })
    .await
}
