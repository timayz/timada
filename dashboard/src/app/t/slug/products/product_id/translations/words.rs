use timada_catalog::CatalogError;
use topcoat::{
    Result,
    context::Cx,
    router::{content::Form, error::SeeOther, route},
};

use super::super::{Tab, act};
use crate::ui::translations::WordsForm;

/// Words the product's options and facts in a language. Each word is the
/// whole shop's: the entry is written once, and only where it changed.
#[route(POST)]
pub async fn words(cx: &Cx, Form(form): Form<WordsForm>) -> Result<SeeOther> {
    act(
        cx,
        Tab::Translations,
        "Words saved, for every product that carries them.",
        async |catalog, actor, _id| {
            for (term, wording) in form.words() {
                catalog.word(actor, &form.language, term, wording).await?;
            }
            Ok::<(), CatalogError>(())
        },
    )
    .await
}
