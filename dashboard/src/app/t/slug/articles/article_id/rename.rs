//! `POST /t/{slug}/articles/{article_id}/rename`: moves the article to
//! another address; the old one redirects to it, and is free for another
//! article.

use serde::Deserialize;
use topcoat::{
    Result,
    context::Cx,
    router::{content::Form, error::SeeOther, route},
};

use super::act;

#[derive(Deserialize)]
pub struct RenameForm {
    slug: String,
}

#[route(POST)]
pub async fn rename(cx: &Cx, Form(form): Form<RenameForm>) -> Result<SeeOther> {
    act(cx, "Address changed.", async |blog, actor, id| {
        blog.rename_article(actor, id, &form.slug).await
    })
    .await
}
