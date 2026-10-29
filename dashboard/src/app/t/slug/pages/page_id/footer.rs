//! `POST /t/{slug}/pages/{page_id}/footer`: lists the page in the
//! storefront's footer at a position, or takes it out.

use serde::Deserialize;
use timada_pages::PagesError;
use topcoat::{
    Result,
    context::Cx,
    router::{content::Form, error::SeeOther, route},
};

use super::act;

#[derive(Deserialize)]
pub struct FooterForm {
    /// `on` when ticked; absent otherwise.
    #[serde(default)]
    in_footer: Option<String>,
    /// Empty is 0.
    #[serde(default)]
    position: String,
}

#[route(POST)]
pub async fn footer(cx: &Cx, Form(form): Form<FooterForm>) -> Result<SeeOther> {
    act(cx, "Footer saved.", async |pages, actor, id| {
        if form.in_footer.is_none() {
            return pages.hide_page_from_footer(actor, id).await;
        }
        let position = match form.position.trim() {
            "" => 0,
            value => value.parse().map_err(|_| PagesError::InvalidPosition)?,
        };
        pages.show_page_in_footer(actor, id, position).await
    })
    .await
}
