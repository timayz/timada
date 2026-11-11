use std::sync::Arc;

use bytes::Bytes;
use timada_catalog::{AddImage, new_image_id};
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

use super::super::{ProductId, Tab, act, product_tab_url};
use crate::app::t::slug::require_tenant;
use crate::media::Media;

/// Takes an uploaded picture: renders it into the two sizes the shop keeps,
/// writes them, then records it. A commit that fails takes the files with
/// it, so the gallery never points at nothing.
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
        Ok(see_other(product_tab_url(
            cx,
            &tenant_slug,
            &product_id,
            Tab::Pictures,
            Some(code),
            None,
        )))
    };

    // The body has to be read to the end whatever happens to it.
    let mut bytes = Bytes::new();
    let mut alt = String::new();
    while let Some(field) = multipart.next_field().await? {
        match field.name().map(str::to_owned).as_deref() {
            Some("image") => bytes = field.bytes().await?,
            Some("alt") => alt = field.text().await?,
            _ => {}
        }
    }
    if bytes.is_empty() {
        return refuse("no_image");
    }
    // What the bytes are, not what the file was called: a renamed executable
    // is still not a picture.
    let media = app_context::<Arc<Media>>(cx).clone();
    let rendered = match media
        .render(bytes)
        .await
        .map_err(topcoat::Error::from_anyhow)?
    {
        Ok(rendered) => rendered,
        Err(refusal) => return refuse(refusal.code()),
    };
    let image_id = new_image_id();
    act(
        cx,
        Tab::Pictures,
        "Picture added.",
        async |catalog, actor, id| {
            // On a majority of nodes before it is recorded, like the event that
            // records it: a node going down takes no picture with it.
            let picture = media
                .keep(&tenant_id, &image_id, rendered)
                .await
                .map_err(timada_catalog::CatalogError::Other)?;
            let added = catalog
                .add_image(
                    actor,
                    id,
                    AddImage {
                        image_id: image_id.clone(),
                        picture,
                        alt: alt.clone(),
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
