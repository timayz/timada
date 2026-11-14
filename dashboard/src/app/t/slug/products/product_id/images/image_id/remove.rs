use topcoat::{
    Result,
    context::Cx,
    router::{error::SeeOther, path_param, route},
};

use super::ImageId;
use crate::app::t::slug::products::product_id::{Tab, act};

/// Takes a picture off the product. Its files are deleted on every node as
/// each follows the stream ([`crate::media::subscription`]), this one too:
/// deleted here at once, the file was missing while the store still knew the
/// picture, which the fetcher takes for a picture lost everywhere (#528).
#[route(POST)]
pub async fn remove(cx: &Cx) -> Result<SeeOther> {
    let image_id = path_param::<ImageId>(cx).to_owned();
    act(
        cx,
        Tab::Pictures,
        "Picture removed.",
        async |catalog, actor, id| {
            catalog.remove_image(actor, id, &image_id).await?;
            Ok(())
        },
    )
    .await
}
