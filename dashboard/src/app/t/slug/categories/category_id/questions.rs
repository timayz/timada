use topcoat::{
    Result,
    context::Cx,
    router::{content::Form, error::SeeOther, route},
};

use super::{QuestionsForm, act, parse_questions};

#[route(POST)]
pub async fn questions(cx: &Cx, Form(form): Form<QuestionsForm>) -> Result<SeeOther> {
    act(cx, "Questions saved.", async |catalog, actor, id| {
        catalog
            .set_category_questions(actor, id, &parse_questions(&form.questions))
            .await
    })
    .await
}
