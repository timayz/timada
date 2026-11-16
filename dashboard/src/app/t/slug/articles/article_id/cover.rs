//! `POST /t/{slug}/articles/{article_id}/cover` (`image_id=…`): picks the
//! cover among the article's pictures; an empty `image_id` leaves it
//! without one.

use serde::Deserialize;
use topcoat::{
    Result,
    context::Cx,
    router::{content::Form, error::SeeOther, route},
};

use super::act;

#[derive(Deserialize)]
pub struct CoverForm {
    #[serde(default)]
    image_id: String,
}

#[route(POST)]
pub async fn cover(cx: &Cx, Form(form): Form<CoverForm>) -> Result<SeeOther> {
    let image_id = Some(form.image_id.trim()).filter(|id| !id.is_empty());
    let message = if image_id.is_some() {
        "Cover set."
    } else {
        "Cover taken off."
    };
    act(cx, message, async |blog, actor, id| {
        blog.set_article_cover(actor, id, image_id).await
    })
    .await
}
