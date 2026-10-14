use topcoat::{
    Result,
    context::Cx,
    router::{content::Form, error::SeeOther, route},
};

use super::{RenameForm, act};

#[route(POST)]
pub async fn rename(cx: &Cx, Form(form): Form<RenameForm>) -> Result<SeeOther> {
    act(cx, "Category renamed.", async |catalog, actor, id| {
        catalog.rename_category(actor, id, &form.name).await
    })
    .await
}
