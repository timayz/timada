use topcoat::{
    Result,
    context::Cx,
    router::{content::Form, error::SeeOther, route},
};

use super::{RejectForm, act};

#[route(POST)]
pub async fn reject(cx: &Cx, Form(form): Form<RejectForm>) -> Result<SeeOther> {
    act(cx, "Review rejected.", async |reviews, actor, id| {
        reviews.reject_review(actor, id, &form.reason).await
    })
    .await
}
