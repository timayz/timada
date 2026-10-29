//! `POST /t/{slug}/pages/{page_id}/upload`: a picture for the page's
//! content. Kept like a product picture (see [`crate::media`]): on a
//! majority of nodes before it is recorded.

use std::sync::Arc;

use timada_catalog::MAX_IMAGE_BYTES;
use timada_pages::{AddPageImage, PagesError, new_page_image_id};
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
use crate::media::Media;

/// Takes an uploaded picture: works out what it is, writes the file, then
/// records it. A commit that fails takes the file with it.
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
    let mut bytes: Vec<u8> = Vec::new();
    while let Some(field) = multipart.next_field().await? {
        if field.name() == Some("image") {
            bytes = field.bytes().await?.to_vec();
        }
    }
    if bytes.is_empty() {
        return refuse("no_image");
    }
    if bytes.len() as u64 > MAX_IMAGE_BYTES {
        return refuse("image_too_large");
    }
    // What the bytes are, not what the file was called.
    let Some(sniffed) = Media::sniff(&bytes) else {
        return refuse("unsupported_image");
    };

    let media = app_context::<Arc<Media>>(cx).clone();
    let image_id = new_page_image_id();
    let file = format!("{image_id}.{}", sniffed.ext);
    act(
        cx,
        "Picture added: copy its line into the content.",
        async |pages, actor, id| {
            media
                .store(&tenant_id, &image_id, sniffed.ext, &bytes)
                .await
                .map_err(PagesError::Other)?;
            if let Err(err) = media.replicate(&tenant_id, &file, bytes.clone()).await {
                media.remove(&tenant_id, &file).await;
                return Err(PagesError::Other(err));
            }
            let added = pages
                .add_page_image(
                    actor,
                    id,
                    AddPageImage {
                        image_id: image_id.clone(),
                        ext: sniffed.ext.to_owned(),
                        size: bytes.len() as u64,
                        width: sniffed.width,
                        height: sniffed.height,
                    },
                )
                .await;
            if added.is_err() {
                media.remove(&tenant_id, &file).await;
            }
            added
        },
    )
    .await
}
