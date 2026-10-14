use topcoat::{
    Result,
    context::Cx,
    router::{content::Form, error::SeeOther, route},
};

use super::AttributeForm;
use crate::app::t::slug::products::product_id::act;

#[route(POST)]
pub async fn remove(cx: &Cx, Form(form): Form<AttributeForm>) -> Result<SeeOther> {
    act(cx, "Fact removed.", async |catalog, actor, id| {
        catalog.remove_attribute(actor, id, &form.name).await
    })
    .await
}
