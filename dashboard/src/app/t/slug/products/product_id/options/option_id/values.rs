use topcoat::{
    Result,
    context::Cx,
    router::{content::Form, error::SeeOther, path_param, route},
};

use super::OptionId;
use crate::app::t::slug::products::product_id::act;
use crate::app::t::slug::products::product_id::options::define::{DefineForm, lines};

/// Replaces the values an option takes. The name comes along in the same
/// form, so a merchant edits both at once.
#[route(POST)]
pub async fn values(cx: &Cx, Form(form): Form<DefineForm>) -> Result<SeeOther> {
    let option_id = path_param::<OptionId>(cx).to_owned();
    act(cx, "Option saved.", async |catalog, actor, id| {
        catalog
            .rename_option(actor, id, &option_id, &form.name)
            .await?;
        catalog
            .change_option_values(actor, id, &option_id, lines(&form.values))
            .await
    })
    .await
}
