use timada_catalog::TranslationKind;
use topcoat::{
    Result,
    context::Cx,
    router::{content::Form, error::SeeOther, route},
};

use super::act;
use crate::ui::translations::TranslationForm;

#[route(POST)]
pub async fn save(cx: &Cx, Form(form): Form<TranslationForm>) -> Result<SeeOther> {
    act(cx, "Translation saved.", async |catalog, actor, id| {
        catalog
            .translate(
                actor,
                TranslationKind::Category,
                id,
                &form.language,
                form.text(),
            )
            .await
    })
    .await
}
