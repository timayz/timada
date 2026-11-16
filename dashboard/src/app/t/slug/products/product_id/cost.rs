use timada_catalog::{CatalogError, VariantCost, parse_amount};
use topcoat::{
    Result,
    context::Cx,
    router::{content::Form, error::SeeOther, route},
};

use super::margin::COST_FIELD_PREFIX;
use super::{CostForm, Tab, act};

/// An amount as typed, `None` for a field left empty.
fn optional_amount(text: &str) -> std::result::Result<Option<u64>, CatalogError> {
    match text.trim() {
        "" => Ok(None),
        text => parse_amount(text).map(Some),
    }
}

/// Says what the shop pays for one of the product, or of each of its
/// combinations: all at once when the one field for them all is filled
/// in, each from its own otherwise. A field left empty forgets the cost.
#[route(POST)]
pub async fn cost(cx: &Cx, Form(form): Form<CostForm>) -> Result<SeeOther> {
    act(
        cx,
        Tab::Margin,
        "Costs saved.",
        async |catalog, actor, id| {
            if let Some(every) = optional_amount(&form.every)? {
                return catalog.set_every_cost(actor, id, Some(every)).await;
            }
            let mut costs = Vec::new();
            for (field, typed) in &form.fields {
                if let Some(variant_id) = field.strip_prefix(COST_FIELD_PREFIX) {
                    costs.push(VariantCost {
                        variant_id: variant_id.to_owned(),
                        amount_cents: optional_amount(typed)?,
                    });
                }
            }
            if costs.is_empty() {
                // A product sold as is posts its one cost.
                let Some(typed) = &form.cost else {
                    return Ok(());
                };
                return catalog.set_cost(actor, id, optional_amount(typed)?).await;
            }
            catalog
                .set_variant_costs(actor, id, &costs)
                .await
                .map(|_| ())
        },
    )
    .await
}
