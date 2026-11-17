use timada_catalog::TranslationKind;
use topcoat::{
    Result,
    context::Cx,
    router::{content::Form, error::SeeOther, route},
};

use super::super::{Tab, act};
use crate::ui::translations::TranslationLanguageForm;

#[route(POST)]
pub async fn remove(cx: &Cx, Form(form): Form<TranslationLanguageForm>) -> Result<SeeOther> {
    act(
        cx,
        Tab::Translations,
        "Translation removed: visitors read the main wording.",
        async |catalog, actor, id| {
            catalog
                .drop_translation(actor, TranslationKind::Product, id, &form.language)
                .await
        },
    )
    .await
}
