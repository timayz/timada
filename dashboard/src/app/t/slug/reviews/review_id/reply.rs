use topcoat::{
    Result,
    context::Cx,
    router::{content::Form, error::SeeOther, route},
};

use super::{ReplyForm, act};

#[route(POST)]
pub async fn reply(cx: &Cx, Form(form): Form<ReplyForm>) -> Result<SeeOther> {
    act(cx, "Reply saved.", async |reviews, actor, id| {
        reviews.reply_to_review(actor, id, &form.body).await
    })
    .await
}
