use topcoat::{Result, context::Cx, router::error::SeeOther, router::route};

use super::act;

#[route(POST)]
pub async fn reactivate(cx: &Cx) -> Result<SeeOther> {
    act(cx, "Offer reactivated.", async |discounts, actor, id| {
        discounts.reactivate_offer(actor, id).await
    })
    .await
}
