use topcoat::{
    Result,
    context::Cx,
    router::{content::Form, error::SeeOther, route},
};

use super::{AiWritingForm, Tab, act};

/// Says whether a machine wrote the product's name and its description.
#[route(POST)]
pub async fn ai_writing(cx: &Cx, Form(form): Form<AiWritingForm>) -> Result<SeeOther> {
    act(cx, Tab::Shopping, "Saved.", async |catalog, actor, id| {
        catalog
            .declare_ai_writing(actor, id, form.title.is_some(), form.description.is_some())
            .await
    })
    .await
}
