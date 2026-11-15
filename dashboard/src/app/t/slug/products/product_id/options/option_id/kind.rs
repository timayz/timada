use topcoat::{
    Result,
    context::Cx,
    router::{content::Form, error::SeeOther, route},
};

use super::{KindForm, option_id};
use crate::app::t::slug::products::product_id::{Tab, act};

/// Says what an option is to a shopping feed: a colour, a size, a material,
/// a pattern, or none of them when the field comes back empty.
#[route(POST)]
pub async fn kind(cx: &Cx, Form(form): Form<KindForm>) -> Result<SeeOther> {
    let option_id = option_id(cx).to_owned();
    act(
        cx,
        Tab::Shopping,
        "Option kind saved.",
        async |catalog, actor, id| {
            catalog
                .set_option_kind(actor, id, &option_id, Some(&form.kind))
                .await
        },
    )
    .await
}
