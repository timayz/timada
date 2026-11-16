//! `POST /t/{slug}/articles/{article_id}/seo`: what search engines show for the
//! article.

use serde::Deserialize;
use topcoat::{
    Result,
    context::Cx,
    router::{content::Form, error::SeeOther, route},
};

use super::act;

#[derive(Deserialize)]
pub struct SeoForm {
    #[serde(default)]
    meta_title: String,
    #[serde(default)]
    meta_description: String,
}

#[route(POST)]
pub async fn seo(cx: &Cx, Form(form): Form<SeoForm>) -> Result<SeeOther> {
    act(cx, "Saved for search engines.", async |blog, actor, id| {
        blog.set_article_seo(actor, id, &form.meta_title, &form.meta_description)
            .await
    })
    .await
}
