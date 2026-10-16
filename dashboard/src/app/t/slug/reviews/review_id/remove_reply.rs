use topcoat::{
    Result,
    context::Cx,
    router::{error::SeeOther, route},
};

use super::act;

#[route(POST)]
pub async fn remove_reply(cx: &Cx) -> Result<SeeOther> {
    act(cx, "Reply deleted.", async |reviews, actor, id| {
        reviews.remove_review_reply(actor, id).await
    })
    .await
}
