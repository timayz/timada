use topcoat::{
    Result,
    context::Cx,
    router::{error::SeeOther, route},
};

use super::act;

#[route(POST)]
pub async fn unpublish(cx: &Cx) -> Result<SeeOther> {
    act(cx, "Product unpublished.", async |catalog, actor, id| {
        catalog.unpublish_product(actor, id).await
    })
    .await
}
