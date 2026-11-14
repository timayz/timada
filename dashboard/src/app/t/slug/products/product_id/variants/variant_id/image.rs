use topcoat::{
    Result,
    context::Cx,
    router::{content::Form, error::SeeOther, route},
};

use super::{ImageForm, variant_id};
use crate::app::t::slug::products::product_id::{Tab, act};

/// Picks which picture a variant shows, or none when the menu comes back
/// empty.
#[route(POST)]
pub async fn image(cx: &Cx, Form(form): Form<ImageForm>) -> Result<SeeOther> {
    let variant_id = variant_id(cx).to_owned();
    act(
        cx,
        Tab::Variants,
        "Variant picture set.",
        async |catalog, actor, id| {
            let image_id = form.image_id.trim();
            catalog
                .image_variant(
                    actor,
                    id,
                    &variant_id,
                    (!image_id.is_empty()).then_some(image_id),
                )
                .await
        },
    )
    .await
}
