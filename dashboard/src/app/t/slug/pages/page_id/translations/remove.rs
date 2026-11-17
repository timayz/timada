use timada_pages::TextKind;
use topcoat::{
    Result,
    context::Cx,
    router::{content::Form, error::SeeOther, route},
};

use super::act;
use crate::ui::translations::TranslationLanguageForm;

#[route(POST)]
pub async fn remove(cx: &Cx, Form(form): Form<TranslationLanguageForm>) -> Result<SeeOther> {
    act(
        cx,
        "Translation removed: visitors read the main wording.",
        async |pages, actor, id| {
            pages
                .drop_text_translation(actor, TextKind::Page, id, &form.language)
                .await
        },
    )
    .await
}
