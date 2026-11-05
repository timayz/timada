//! `POST /t/{slug}/announcements/{announcement_id}/remove`: deletes the
//! message for good.

use topcoat::{Result, context::Cx, router::error::SeeOther, router::route};

use super::act;

#[route(POST)]
pub async fn remove(cx: &Cx) -> Result<SeeOther> {
    act(cx, "Announcement deleted.", async |pages, actor, id| {
        pages.remove_announcement(actor, id).await
    })
    .await
}
