use topcoat::{
    Result,
    context::Cx,
    router::{content::Form, error::SeeOther, path_param, route},
};

use super::{LabelForm, VariantId};
use crate::app::t::slug::products::product_id::act;

/// Sets what a shopper reads for a variant instead of its values, or goes
/// back to the values when the field comes back empty. The values stay as
/// they are, so an import that named them still finds the variant.
#[route(POST)]
pub async fn label(cx: &Cx, Form(form): Form<LabelForm>) -> Result<SeeOther> {
    let variant_id = path_param::<VariantId>(cx).to_owned();
    act(cx, "Variant label saved.", async |catalog, actor, id| {
        let label = form.label.trim();
        catalog
            .label_variant(actor, id, &variant_id, (!label.is_empty()).then_some(label))
            .await
    })
    .await
}
