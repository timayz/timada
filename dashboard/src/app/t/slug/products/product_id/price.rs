use timada_catalog::parse_amount;
use topcoat::{
    Result,
    context::Cx,
    router::{content::Form, error::SeeOther, route},
};

use super::{PriceForm, act};

#[route(POST)]
pub async fn price(cx: &Cx, Form(form): Form<PriceForm>) -> Result<SeeOther> {
    act(cx, "Price set.", async |catalog, actor, id| {
        let amount = parse_amount(&form.amount)?;
        catalog
            .price_product(actor, id, amount, &form.currency)
            .await
    })
    .await
}
