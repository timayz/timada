//! `POST /t/{slug}/pages/{page_id}/unpublish`: back to a draft, off the
//! storefront and its footer.

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
        async |pages, actor, id| pages.unpublish_page(actor, id).await,
    )
    .await
}
