use topcoat::{
    Result,
    context::Cx,
    router::{content::Form, error::SeeOther, route},
};

use super::{PhotoForm, act};

/// Takes one photo off the review; every node deletes its file once the
/// event reaches it.
#[route(POST)]
pub async fn remove_photo(cx: &Cx, Form(form): Form<PhotoForm>) -> Result<SeeOther> {
    act(cx, "Photo removed.", async |reviews, actor, id| {
        reviews.remove_review_photo(actor, id, &form.image_id).await
    })
    .await
}
