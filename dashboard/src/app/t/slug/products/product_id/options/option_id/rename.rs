use serde::Deserialize;
use topcoat::{
    Result,
    context::Cx,
    router::{content::Form, error::SeeOther, path_param, route},
};

use super::OptionId;
use crate::app::t::slug::products::product_id::act;

#[derive(Deserialize)]
pub struct RenameForm {
    pub name: String,
}

#[route(POST)]
pub async fn rename(cx: &Cx, Form(form): Form<RenameForm>) -> Result<SeeOther> {
    let option_id = path_param::<OptionId>(cx).to_owned();
    act(cx, "Option renamed.", async |catalog, actor, id| {
        catalog
            .rename_option(actor, id, &option_id, &form.name)
            .await
    })
    .await
}
