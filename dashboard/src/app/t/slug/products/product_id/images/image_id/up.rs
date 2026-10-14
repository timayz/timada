use topcoat::{
    Result,
    context::Cx,
    router::{error::SeeOther, path_param, route},
};

use super::{ImageId, moved};
use crate::app::t::slug::products::product_id::act;

/// Moves the picture one place earlier in the gallery.
#[route(POST)]
pub async fn up(cx: &Cx) -> Result<SeeOther> {
    let image_id = path_param::<ImageId>(cx).to_owned();
    act(cx, "Gallery reordered.", async |catalog, actor, id| {
        let Some(product) = catalog.load_product(id).await? else {
            return Ok(());
        };
        match moved(&product.images, &image_id, -1) {
            Some(order) => catalog.reorder_images(actor, id, order).await,
            None => Ok(()),
        }
    })
    .await
}
