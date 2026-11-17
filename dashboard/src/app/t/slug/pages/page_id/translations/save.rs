use timada_pages::TextKind;
use topcoat::{
    Result,
    context::Cx,
    router::{content::Form, error::SeeOther, route},
};

use super::act;
use crate::ui::translations::TextTranslationForm;

#[route(POST)]
pub async fn save(cx: &Cx, Form(form): Form<TextTranslationForm>) -> Result<SeeOther> {
    act(cx, "Translation saved.", async |pages, actor, id| {
        pages
            .translate_text(actor, TextKind::Page, id, &form.language, form.text())
            .await
    })
    .await
}
