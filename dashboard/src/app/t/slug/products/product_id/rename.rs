use topcoat::{
    Result,
    context::Cx,
    router::{content::Form, error::SeeOther, route},
};

use super::{RenameForm, Tab, act};

#[route(POST)]
pub async fn rename(cx: &Cx, Form(form): Form<RenameForm>) -> Result<SeeOther> {
    act(
        cx,
        Tab::Overview,
        "Product renamed.",
        async |catalog, actor, id| catalog.rename_product(actor, id, &form.name).await,
    )
    .await
}
