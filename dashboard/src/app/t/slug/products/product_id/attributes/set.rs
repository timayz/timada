use topcoat::{
    Result,
    context::Cx,
    router::{content::Form, error::SeeOther, route},
};

use super::AttributeForm;
use crate::app::t::slug::products::product_id::act;

#[route(POST)]
pub async fn set(cx: &Cx, Form(form): Form<AttributeForm>) -> Result<SeeOther> {
    act(cx, "Fact saved.", async |catalog, actor, id| {
        catalog
            .set_attribute(actor, id, &form.name, &form.value)
            .await
    })
    .await
}
