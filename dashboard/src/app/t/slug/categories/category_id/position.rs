use topcoat::{
    Result,
    context::Cx,
    router::{content::Form, error::SeeOther, route},
};

use super::{PositionForm, act};

#[route(POST)]
pub async fn position(cx: &Cx, Form(form): Form<PositionForm>) -> Result<SeeOther> {
    act(cx, "Position set.", async |catalog, actor, id| {
        catalog.position_category(actor, id, form.position).await
    })
    .await
}
