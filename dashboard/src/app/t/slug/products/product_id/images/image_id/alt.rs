use topcoat::{
    Result,
    context::Cx,
    router::{content::Form, error::SeeOther, route},
};

use super::{AltForm, image_id};
use crate::app::t::slug::products::product_id::{Tab, act};

#[route(POST)]
pub async fn alt(cx: &Cx, Form(form): Form<AltForm>) -> Result<SeeOther> {
    let image_id = image_id(cx).to_owned();
    act(
        cx,
        Tab::Pictures,
        "Picture description saved.",
        async |catalog, actor, id| {
            catalog
                .change_image_alt(actor, id, &image_id, &form.alt)
                .await
        },
    )
    .await
}
