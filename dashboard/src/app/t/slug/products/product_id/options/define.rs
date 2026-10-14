use serde::Deserialize;
use topcoat::{
    Result,
    context::Cx,
    router::{content::Form, error::SeeOther, route},
};

use crate::app::t::slug::products::product_id::act;

#[derive(Deserialize)]
pub struct DefineForm {
    pub name: String,
    /// One value a line, as the textarea posts them.
    #[serde(default)]
    pub values: String,
}

/// The lines of a textarea as a list of values.
pub fn lines(values: &str) -> Vec<String> {
    values
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect()
}

#[route(POST)]
pub async fn define(cx: &Cx, Form(form): Form<DefineForm>) -> Result<SeeOther> {
    act(cx, "Option added.", async |catalog, actor, id| {
        catalog
            .define_option(actor, id, &form.name, lines(&form.values))
            .await
            .map(|_| ())
    })
    .await
}
