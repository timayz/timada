//! `POST /t/{slug}/pages/{page_id}/publish`: shows the page on the storefront.

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
        "Published: the storefront shows it in a moment.",
        async |pages, actor, id| pages.publish_page(actor, id).await,
    )
    .await
}
