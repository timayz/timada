use topcoat::{
    Result,
    context::Cx,
    router::{content::Form, error::SeeOther, route},
};

use super::{IdentifyForm, variant_id};
use crate::app::t::slug::products::product_id::{Tab, act};

/// Sets the GTIN and the part number of a variant; an empty field clears
/// it.
#[route(POST)]
pub async fn identify(cx: &Cx, Form(form): Form<IdentifyForm>) -> Result<SeeOther> {
    let variant_id = variant_id(cx).to_owned();
    act(
        cx,
        Tab::Shopping,
        "Variant identifiers saved.",
        async |catalog, actor, id| {
            catalog
                .identify_variant(actor, id, &variant_id, Some(&form.gtin), Some(&form.mpn))
                .await
        },
    )
    .await
}
