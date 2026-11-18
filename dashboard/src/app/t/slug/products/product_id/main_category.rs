use topcoat::{
    Result,
    context::Cx,
    router::{content::Form, error::SeeOther, route},
};

use super::categorize::CategorizeForm;
use super::{Tab, act};

/// Picks the category the product's address is under.
#[route(POST)]
pub async fn main_category(cx: &Cx, Form(form): Form<CategorizeForm>) -> Result<SeeOther> {
    act(
        cx,
        Tab::Categories,
        "Main category set.",
        async |catalog, actor, id| {
            catalog
                .set_main_category(actor, id, Some(form.category_id.trim()))
                .await
        },
    )
    .await
}
