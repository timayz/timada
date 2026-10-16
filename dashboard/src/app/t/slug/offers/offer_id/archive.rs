use topcoat::{Result, context::Cx, router::error::SeeOther, router::route};

use super::act;

#[route(POST)]
pub async fn archive(cx: &Cx) -> Result<SeeOther> {
    act(cx, "Offer archived.", async |discounts, actor, id| {
        discounts.archive_offer(actor, id).await
    })
    .await
}
