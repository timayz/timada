//! `POST /t/{slug}/pages/{page_id}/rename`: moves the page to another
//! address; the old one is free for another page.

use serde::Deserialize;
use topcoat::{
    Result,
    context::Cx,
    router::{content::Form, error::SeeOther, route},
};

use super::act;

#[derive(Deserialize)]
pub struct RenameForm {
    slug: String,
}

#[route(POST)]
pub async fn rename(cx: &Cx, Form(form): Form<RenameForm>) -> Result<SeeOther> {
    act(cx, "Address changed.", async |pages, actor, id| {
        pages.rename_page(actor, id, &form.slug).await
    })
    .await
}
