use topcoat::{
    Result,
    context::Cx,
    router::{error::SeeOther, route},
};

use super::variant_id;
use crate::app::t::slug::products::product_id::{Tab, act};

#[route(POST)]
pub async fn remove(cx: &Cx) -> Result<SeeOther> {
    let variant_id = variant_id(cx).to_owned();
    act(
        cx,
        Tab::Variants,
        "Variant removed.",
        async |catalog, actor, id| catalog.remove_variant(actor, id, &variant_id).await,
    )
    .await
}
