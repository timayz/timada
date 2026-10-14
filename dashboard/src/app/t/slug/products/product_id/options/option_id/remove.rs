use topcoat::{
    Result,
    context::Cx,
    router::{error::SeeOther, path_param, route},
};

use super::OptionId;
use crate::app::t::slug::products::product_id::act;

#[route(POST)]
pub async fn remove(cx: &Cx) -> Result<SeeOther> {
    let option_id = path_param::<OptionId>(cx).to_owned();
    act(cx, "Option removed.", async |catalog, actor, id| {
        catalog.remove_option(actor, id, &option_id).await
    })
    .await
}
