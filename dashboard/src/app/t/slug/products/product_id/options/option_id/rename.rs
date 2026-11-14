use serde::Deserialize;
use topcoat::{
    Result,
    context::Cx,
    router::{content::Form, error::SeeOther, route},
};

use super::option_id;
use crate::app::t::slug::products::product_id::{Tab, act};

#[derive(Deserialize)]
pub struct RenameForm {
    pub name: String,
}

#[route(POST)]
pub async fn rename(cx: &Cx, Form(form): Form<RenameForm>) -> Result<SeeOther> {
    let option_id = option_id(cx).to_owned();
    act(
        cx,
        Tab::Variants,
        "Option renamed.",
        async |catalog, actor, id| {
            catalog
                .rename_option(actor, id, &option_id, &form.name)
                .await
        },
    )
    .await
}
