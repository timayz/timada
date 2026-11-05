//! `POST /t/{slug}/announcements/{announcement_id}/move-to`: moves the
//! message to `position` (0 is first) in the bar's order.

use serde::Deserialize;
use topcoat::{
    Result,
    context::Cx,
    router::{content::Form, error::SeeOther, route},
};

use super::act;

#[derive(Deserialize)]
pub struct MoveForm {
    position: u32,
}

#[route(POST)]
pub async fn move_to(cx: &Cx, Form(form): Form<MoveForm>) -> Result<SeeOther> {
    act(cx, "Order saved.", async |pages, actor, id| {
        pages.move_announcement(actor, id, form.position).await
    })
    .await
}
