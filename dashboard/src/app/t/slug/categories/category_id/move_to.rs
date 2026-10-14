use topcoat::{
    Result,
    context::Cx,
    router::{content::Form, error::SeeOther, route},
};

use super::{MoveForm, act};

/// Puts the category under another one, or at the top level when the parent
/// field comes back empty.
#[route(POST)]
pub async fn move_to(cx: &Cx, Form(form): Form<MoveForm>) -> Result<SeeOther> {
    act(cx, "Category moved.", async |catalog, actor, id| {
        let parent = form
            .parent_id
            .as_deref()
            .map(str::trim)
            .filter(|p| !p.is_empty());
        catalog.move_category(actor, id, parent).await
    })
    .await
}
