//! `POST /t/{slug}/articles/{article_id}/remove_image` (`image_id=…`): takes a
//! picture off the article, and its cover with it when it was this one. Its files are deleted on every node as each
//! follows the stream ([`crate::media::subscription`]).

use serde::Deserialize;
use topcoat::{
    Result,
    context::Cx,
    router::{content::Form, error::SeeOther, route},
};

use super::act;

#[derive(Deserialize)]
pub struct RemoveImageForm {
    image_id: String,
}

#[route(POST)]
pub async fn remove_image(cx: &Cx, Form(form): Form<RemoveImageForm>) -> Result<SeeOther> {
    act(cx, "Picture removed.", async |blog, actor, id| {
        blog.remove_article_image(actor, id, &form.image_id).await?;
        Ok(())
    })
    .await
}
