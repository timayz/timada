use topcoat::{
    Result,
    context::Cx,
    router::{content::Form, error::SeeOther, path_param, route},
};

use super::OptionId;
use crate::app::t::slug::products::product_id::{Tab, act};

#[derive(serde::Deserialize)]
pub struct VisibilityForm {
    /// The value the shop sells; empty lets shoppers choose.
    value: String,
}

/// Hides an option from shoppers at the value picked, or lets them choose it
/// again.
#[route(POST)]
pub async fn visibility(cx: &Cx, Form(form): Form<VisibilityForm>) -> Result<SeeOther> {
    let option_id = path_param::<OptionId>(cx).to_owned();
    let message = if form.value.is_empty() {
        "Shoppers choose this option again."
    } else {
        "Option hidden from shoppers."
    };
    act(cx, Tab::Variants, message, async |catalog, actor, id| {
        if form.value.is_empty() {
            catalog.show_option(actor, id, &option_id).await
        } else {
            catalog
                .hide_option(actor, id, &option_id, &form.value)
                .await
        }
    })
    .await
}
