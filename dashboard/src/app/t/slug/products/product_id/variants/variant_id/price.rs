use timada_catalog::parse_amount;
use topcoat::{
    Result,
    context::Cx,
    router::{content::Form, error::SeeOther, path_param, route},
};

use super::{PriceForm, VariantId};
use crate::app::t::slug::products::product_id::act;

#[route(POST)]
pub async fn price(cx: &Cx, Form(form): Form<PriceForm>) -> Result<SeeOther> {
    let variant_id = path_param::<VariantId>(cx).to_owned();
    act(cx, "Variant price set.", async |catalog, actor, id| {
        let amount = parse_amount(&form.amount)?;
        catalog
            .price_variant(actor, id, &variant_id, amount, &form.currency)
            .await
    })
    .await
}
