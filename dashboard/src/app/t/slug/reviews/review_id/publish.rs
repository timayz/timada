use topcoat::{Result, context::Cx, router::error::SeeOther, router::route};

use super::act;

#[route(POST)]
pub async fn publish(cx: &Cx) -> Result<SeeOther> {
    act(cx, "Review published.", async |reviews, actor, id| {
        reviews.publish_review(actor, id).await
    })
    .await
}
