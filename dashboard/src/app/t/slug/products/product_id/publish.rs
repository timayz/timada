use topcoat::{
    Result,
    context::Cx,
    router::{error::SeeOther, route},
};

use super::{Tab, act};

#[route(POST)]
pub async fn publish(cx: &Cx) -> Result<SeeOther> {
    act(
        cx,
        Tab::Overview,
        "Product published.",
        async |catalog, actor, id| catalog.publish_product(actor, id).await,
    )
    .await
}
