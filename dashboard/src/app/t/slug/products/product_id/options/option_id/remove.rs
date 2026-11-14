use topcoat::{
    Result,
    context::Cx,
    router::{error::SeeOther, route},
};

use super::option_id;
use crate::app::t::slug::products::product_id::{Tab, act};

#[route(POST)]
pub async fn remove(cx: &Cx) -> Result<SeeOther> {
    let option_id = option_id(cx).to_owned();
    act(
        cx,
        Tab::Variants,
        "Option removed.",
        async |catalog, actor, id| catalog.remove_option(actor, id, &option_id).await,
    )
    .await
}
