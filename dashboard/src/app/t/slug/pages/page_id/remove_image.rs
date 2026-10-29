//! `POST /t/{slug}/pages/{page_id}/remove_image` (`image_id=…`): takes a
//! picture off the page and deletes its file, here and, as they follow the
//! stream, on the other nodes ([`crate::media::subscription`]).

use std::sync::Arc;

use serde::Deserialize;
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{content::Form, error::SeeOther, route},
};

use super::act;
use crate::media::Media;

#[derive(Deserialize)]
pub struct RemoveImageForm {
    image_id: String,
}

#[route(POST)]
pub async fn remove_image(cx: &Cx, Form(form): Form<RemoveImageForm>) -> Result<SeeOther> {
    let media = app_context::<Arc<Media>>(cx).clone();
    act(cx, "Picture removed.", async |pages, actor, id| {
        pages.remove_page_image(actor, id, &form.image_id).await?;
        media.remove_image(pages.tenant(), &form.image_id).await;
        Ok(())
    })
    .await
}
