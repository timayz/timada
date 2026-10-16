use topcoat::{Result, context::Cx, router::error::SeeOther, router::route};

use super::act;

#[route(POST)]
pub async fn deactivate(cx: &Cx) -> Result<SeeOther> {
    act(cx, "Offer deactivated.", async |discounts, actor, id| {
        discounts.deactivate_offer(actor, id).await
    })
    .await
}
