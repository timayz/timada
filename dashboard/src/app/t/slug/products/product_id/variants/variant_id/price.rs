use timada_catalog::parse_amount;
use topcoat::{
    Result,
    context::Cx,
    router::{content::Form, error::SeeOther, route},
};

use super::{PriceForm, variant_id};
use crate::app::t::slug::products::product_id::{Tab, act};

#[route(POST)]
pub async fn price(cx: &Cx, Form(form): Form<PriceForm>) -> Result<SeeOther> {
    let variant_id = variant_id(cx).to_owned();
    act(
        cx,
        Tab::Variants,
        "Variant price set.",
        async |catalog, actor, id| {
            let amount = parse_amount(&form.amount)?;
            catalog
                .price_variant(actor, id, &variant_id, amount, &form.currency)
                .await
        },
    )
    .await
}
