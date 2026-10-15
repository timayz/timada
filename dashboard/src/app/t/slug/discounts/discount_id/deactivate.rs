use topcoat::{Result, context::Cx, router::error::SeeOther, router::route};

use super::act;

#[route(POST)]
pub async fn deactivate(cx: &Cx) -> Result<SeeOther> {
    act(cx, "Discount deactivated.", async |discounts, actor, id| {
        discounts.deactivate_discount(actor, id).await
    })
    .await
}
