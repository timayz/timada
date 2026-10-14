use topcoat::{
    Result,
    context::Cx,
    router::{content::Form, error::SeeOther, path_param, route},
};

use super::{SkuForm, VariantId};
use crate::app::t::slug::products::product_id::act;

/// Sets the merchant's code for a variant, or clears it when the field
/// comes back empty.
#[route(POST)]
pub async fn sku(cx: &Cx, Form(form): Form<SkuForm>) -> Result<SeeOther> {
    let variant_id = path_param::<VariantId>(cx).to_owned();
    act(cx, "Variant code saved.", async |catalog, actor, id| {
        let sku = form.sku.trim();
        catalog
            .change_variant_sku(actor, id, &variant_id, (!sku.is_empty()).then_some(sku))
            .await
    })
    .await
}
