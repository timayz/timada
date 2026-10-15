use topcoat::{
    Result,
    context::Cx,
    router::{content::Form, error::SeeOther, route},
};

use super::{MenuForm, act};

#[route(POST)]
pub async fn menu(cx: &Cx, Form(form): Form<MenuForm>) -> Result<SeeOther> {
    let in_menu = form.in_menu.is_some();
    let message = if in_menu {
        "Shown in the menu."
    } else {
        "Hidden from the menu."
    };
    act(cx, message, async |catalog, actor, id| {
        catalog.set_category_in_menu(actor, id, in_menu).await
    })
    .await
}
