use std::sync::Arc;

use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{error::SeeOther, path_param, route},
};

use super::ImageId;
use crate::app::t::slug::products::product_id::act;
use crate::app::t::slug::require_tenant;
use crate::media::Media;

/// Takes a picture off the product, then deletes the file. A storefront a
/// moment behind may ask for it once more and get a 404; the next refresh
/// has it gone.
#[route(POST)]
pub async fn remove(cx: &Cx) -> Result<SeeOther> {
    let tenant_id = require_tenant(cx).await?.tenant.id;
    let image_id = path_param::<ImageId>(cx).to_owned();
    let media = app_context::<Arc<Media>>(cx).clone();
    act(cx, "Picture removed.", async |catalog, actor, id| {
        let file = catalog.load_product(id).await?.and_then(|product| {
            product
                .images
                .iter()
                .find(|image| image.id == image_id)
                .map(|image| image.file())
        });
        catalog.remove_image(actor, id, &image_id).await?;
        if let Some(file) = file {
            media.remove(&tenant_id, &file).await;
        }
        Ok(())
    })
    .await
}
