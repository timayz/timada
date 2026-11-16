//! `POST /t/{slug}/articles/{article_id}/publish`: lists the article on the
//! storefront's blog.

use topcoat::{
    Result,
    context::Cx,
    router::{error::SeeOther, route},
};

use super::act;

#[route(POST)]
pub async fn publish(cx: &Cx) -> Result<SeeOther> {
    act(
        cx,
        "Published: the blog lists it in a moment.",
        async |blog, actor, id| blog.publish_article(actor, id).await,
    )
    .await
}
