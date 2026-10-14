use topcoat::{
    Result,
    context::Cx,
    router::{content::Form, error::SeeOther, path_param, route},
};

use super::{AltForm, ImageId};
use crate::app::t::slug::products::product_id::act;

#[route(POST)]
pub async fn alt(cx: &Cx, Form(form): Form<AltForm>) -> Result<SeeOther> {
    let image_id = path_param::<ImageId>(cx).to_owned();
    act(
        cx,
        "Picture description saved.",
        async |catalog, actor, id| {
            catalog
                .change_image_alt(actor, id, &image_id, &form.alt)
                .await
        },
    )
    .await
}
