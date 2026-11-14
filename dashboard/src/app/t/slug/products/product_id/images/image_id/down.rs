use topcoat::{
    Result,
    context::Cx,
    router::{error::SeeOther, route},
};

use super::{image_id, moved};
use crate::app::t::slug::products::product_id::{Tab, act};

/// Moves the picture one place later in the gallery.
#[route(POST)]
pub async fn down(cx: &Cx) -> Result<SeeOther> {
    let image_id = image_id(cx).to_owned();
    act(
        cx,
        Tab::Pictures,
        "Gallery reordered.",
        async |catalog, actor, id| {
            let Some(product) = catalog.load_product(id).await? else {
                return Ok(());
            };
            match moved(&product.images, &image_id, 1) {
                Some(order) => catalog.reorder_images(actor, id, order).await,
                None => Ok(()),
            }
        },
    )
    .await
}
