//! `POST /t/{slug}/articles/{article_id}/unpublish`: back to a draft, off the
//! blog.

use topcoat::{
    Result,
    context::Cx,
    router::{error::SeeOther, route},
};

use super::act;

#[route(POST)]
pub async fn unpublish(cx: &Cx) -> Result<SeeOther> {
    act(
        cx,
        "Unpublished: it is a draft again.",
        async |blog, actor, id| blog.unpublish_article(actor, id).await,
    )
    .await
}
