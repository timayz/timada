use topcoat::{Result, context::Cx, router::error::SeeOther, router::route};

use super::act;

#[route(POST)]
pub async fn archive(cx: &Cx) -> Result<SeeOther> {
    act(cx, "Discount archived.", async |discounts, actor, id| {
        discounts.archive_discount(actor, id).await
    })
    .await
}
