use topcoat::{
    Result,
    context::Cx,
    router::{error::SeeOther, path_param, route},
};

use super::VariantId;
use crate::app::t::slug::products::product_id::act;

#[route(POST)]
pub async fn remove(cx: &Cx) -> Result<SeeOther> {
    let variant_id = path_param::<VariantId>(cx).to_owned();
    act(cx, "Variant removed.", async |catalog, actor, id| {
        catalog.remove_variant(actor, id, &variant_id).await
    })
    .await
}
