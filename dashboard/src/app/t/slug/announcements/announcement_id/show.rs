//! `POST /t/{slug}/announcements/{announcement_id}/show`: puts the message
//! on the bar (`shown=true`) or takes it off, keeping it (`shown=false`).

use serde::Deserialize;
use topcoat::{
    Result,
    context::Cx,
    router::{content::Form, error::SeeOther, route},
};

use super::act;

#[derive(Deserialize)]
pub struct ShowForm {
    shown: bool,
}

#[route(POST)]
pub async fn show(cx: &Cx, Form(form): Form<ShowForm>) -> Result<SeeOther> {
    if form.shown {
        act(cx, "Announcement shown.", async |pages, actor, id| {
            pages.show_announcement(actor, id).await
        })
        .await
    } else {
        act(cx, "Announcement hidden.", async |pages, actor, id| {
            pages.hide_announcement(actor, id).await
        })
        .await
    }
}
