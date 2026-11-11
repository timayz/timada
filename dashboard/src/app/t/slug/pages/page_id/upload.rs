//! `POST /t/{slug}/pages/{page_id}/upload`: a picture for the page's
//! content. Kept like a product picture (see [`crate::media`]): on a
//! majority of nodes before it is recorded.

use std::sync::Arc;

use bytes::Bytes;
use timada_pages::{AddPageImage, PageError, new_page_image_id};
use timada_tenancy::PERMISSION_PAGES_MANAGE;
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{
        content::multipart::Multipart,
        error::{SeeOther, see_other},
        path_param, route,
    },
};

use super::{PageId, act, page_url};
use crate::app::t::slug::require_tenant;
use crate::media::{Media, Quota, Room, STORAGE_FULL};

/// Takes an uploaded picture: renders it into the two sizes the shop keeps,
/// writes them, then records it. A commit that fails takes the files with
/// it.
#[route(POST)]
pub async fn upload(cx: &Cx, mut multipart: Multipart) -> Result<SeeOther> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_PAGES_MANAGE)
        .await?;
    let tenant_id = access.tenant.id.clone();
    let tenant_slug = access.tenant.slug.clone();
    let page_id = path_param::<PageId>(cx).to_owned();
    let refuse = |code: &str| {
        Ok(see_other(page_url(
            cx,
            &tenant_slug,
            &page_id,
            Some(code),
            None,
        )))
    };

    // The body has to be read to the end whatever happens to it.
    let mut bytes = Bytes::new();
    while let Some(field) = multipart.next_field().await? {
        if field.name() == Some("image") {
            bytes = field.bytes().await?;
        }
    }
    if bytes.is_empty() {
        return refuse("no_image");
    }
    // What the bytes are, not what the file was called.
    let media = app_context::<Arc<Media>>(cx).clone();
    let rendered = match media
        .render(bytes)
        .await
        .map_err(topcoat::Error::from_anyhow)?
    {
        Ok(rendered) => rendered,
        Err(refusal) => return refuse(refusal.code()),
    };
    // Within what the shop's pictures may take.
    let room = app_context::<Arc<Quota>>(cx)
        .room_for(&access.tenant, rendered.picture.bytes())
        .await
        .map_err(topcoat::Error::from_anyhow)?;
    if let Room::Full { .. } = room {
        return refuse(STORAGE_FULL);
    }
    let image_id = new_page_image_id();
    act(
        cx,
        "Picture added: copy its line into the content.",
        async |pages, actor, id| {
            let picture = media
                .keep(&tenant_id, &image_id, rendered)
                .await
                .map_err(PageError::Other)?;
            let added = pages
                .add_page_image(
                    actor,
                    id,
                    AddPageImage {
                        image_id: image_id.clone(),
                        picture,
                    },
                )
                .await;
            if added.is_err() {
                media.remove_image(&tenant_id, &image_id).await;
            }
            added
        },
    )
    .await
}
