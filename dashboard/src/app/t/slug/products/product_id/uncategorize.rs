use topcoat::{
    Result,
    context::Cx,
    router::{content::Form, error::SeeOther, route},
};

use super::act;
use super::categorize::CategorizeForm;

/// Takes the product out of a category, archived or not.
#[route(POST)]
pub async fn uncategorize(cx: &Cx, Form(form): Form<CategorizeForm>) -> Result<SeeOther> {
    act(
        cx,
        "Removed from the category.",
        async |catalog, actor, id| {
            catalog
                .uncategorize_product(actor, id, form.category_id.trim())
                .await
        },
    )
    .await
}
