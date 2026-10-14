use std::sync::Arc;

use timada_catalog::{AddImage, MAX_IMAGE_BYTES, new_image_id};
use timada_tenancy::PERMISSION_PRODUCTS_MANAGE;
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{
        content::multipart::Multipart,
        error::{SeeOther, see_other},
        path_param, route,
    },
};

use super::super::{ProductId, act, product_url};
use crate::app::t::slug::require_tenant;
use crate::media::Media;

/// Takes an uploaded picture: works out what it is, writes the file, then
/// records it. A commit that fails takes the file with it, so the gallery
/// never points at nothing.
#[route(POST)]
pub async fn upload(cx: &Cx, mut multipart: Multipart) -> Result<SeeOther> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_PRODUCTS_MANAGE)
        .await?;
    let tenant_id = access.tenant.id.clone();
    let tenant_slug = access.tenant.slug.clone();
    let product_id = path_param::<ProductId>(cx).to_owned();
    let refuse = |code: &str| {
        Ok(see_other(product_url(
            cx,
            &tenant_slug,
            &product_id,
            Some(code),
            None,
        )))
    };

    // The body has to be read to the end whatever happens to it.
    let mut bytes: Vec<u8> = Vec::new();
    let mut alt = String::new();
    while let Some(field) = multipart.next_field().await? {
        match field.name().map(str::to_owned).as_deref() {
            Some("image") => bytes = field.bytes().await?.to_vec(),
            Some("alt") => alt = field.text().await?,
            _ => {}
        }
    }
    if bytes.is_empty() {
        return refuse("no_image");
    }
    if bytes.len() as u64 > MAX_IMAGE_BYTES {
        return refuse("image_too_large");
    }
    // What the bytes are, not what the file was called: a renamed executable
    // is still not a picture.
    let Some(sniffed) = Media::sniff(&bytes) else {
        return refuse("unsupported_image");
    };

    let media = app_context::<Arc<Media>>(cx).clone();
    let image_id = new_image_id();
    let file = format!("{image_id}.{}", sniffed.ext);
    act(cx, "Picture added.", async |catalog, actor, id| {
        media
            .store(&tenant_id, &image_id, sniffed.ext, &bytes)
            .await
            .map_err(timada_catalog::CatalogError::Other)?;
        // On a majority of nodes before it is recorded, like the event that
        // records it: a node going down takes no picture with it.
        if let Err(err) = media.replicate(&tenant_id, &file, bytes.clone()).await {
            media.remove(&tenant_id, &file).await;
            return Err(timada_catalog::CatalogError::Other(err));
        }
        let added = catalog
            .add_image(
                actor,
                id,
                AddImage {
                    image_id: image_id.clone(),
                    ext: sniffed.ext.to_owned(),
                    size: bytes.len() as u64,
                    width: sniffed.width,
                    height: sniffed.height,
                    alt: alt.clone(),
                },
            )
            .await;
        if added.is_err() {
            media.remove(&tenant_id, &file).await;
        }
        added
    })
    .await
}
