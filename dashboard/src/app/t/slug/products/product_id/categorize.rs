use serde::Deserialize;
use topcoat::{
    Result,
    context::Cx,
    router::{content::Form, error::SeeOther, route},
};

use super::act;

#[derive(Deserialize)]
pub struct CategorizeForm {
    pub category_id: String,
}

/// Files the product under one more category.
#[route(POST)]
pub async fn categorize(cx: &Cx, Form(form): Form<CategorizeForm>) -> Result<SeeOther> {
    act(cx, "Added to the category.", async |catalog, actor, id| {
        catalog
            .categorize_product(actor, id, form.category_id.trim())
            .await
    })
    .await
}
