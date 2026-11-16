use timada_catalog::{CatalogError, parse_amount};
use topcoat::{
    Result,
    context::Cx,
    router::{content::Form, error::SeeOther, route},
};

use super::{OwnCostsForm, Tab, act};

/// Says what sending one and importing one cost for this product; a field
/// left empty goes back to the shop's.
#[route(POST)]
pub async fn own_costs(cx: &Cx, Form(form): Form<OwnCostsForm>) -> Result<SeeOther> {
    act(
        cx,
        Tab::Margin,
        "Costs saved.",
        async |catalog, actor, id| {
            let amount = |text: &str| -> std::result::Result<Option<u64>, CatalogError> {
                match text.trim() {
                    "" => Ok(None),
                    text => parse_amount(text).map(Some),
                }
            };
            catalog
                .set_own_costs(actor, id, amount(&form.shipping)?, amount(&form.import)?)
                .await
        },
    )
    .await
}
