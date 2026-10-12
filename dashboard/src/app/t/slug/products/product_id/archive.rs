use topcoat::{
    Result,
    context::Cx,
    router::{error::SeeOther, route},
};

use super::act;

#[route(POST)]
pub async fn archive(cx: &Cx) -> Result<SeeOther> {
    act(cx, "Product archived.", async |catalog, actor, id| {
        catalog.archive_product(actor, id).await
    })
    .await
}
