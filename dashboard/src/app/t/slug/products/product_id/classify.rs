use topcoat::{
    Result,
    context::Cx,
    router::{content::Form, error::SeeOther, route},
};

use timada_catalog::Classify;

use super::{ClassifyForm, Tab, act};

/// Files the product for a shopping feed; an empty field clears it.
#[route(POST)]
pub async fn classify(cx: &Cx, Form(form): Form<ClassifyForm>) -> Result<SeeOther> {
    let cmd = Classify {
        google_category: Some(form.google_category),
        age_group: Some(form.age_group),
        gender: Some(form.gender),
    };
    act(
        cx,
        Tab::Shopping,
        "Classification saved.",
        async |catalog, actor, id| catalog.classify_product(actor, id, cmd).await,
    )
    .await
}
