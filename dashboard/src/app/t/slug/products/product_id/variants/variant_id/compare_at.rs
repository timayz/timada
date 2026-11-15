use timada_catalog::parse_amount;
use topcoat::{
    Result,
    context::Cx,
    router::{content::Form, error::SeeOther, route},
};

use super::variant_id;
use crate::app::t::slug::products::product_id::{CompareAtForm, Tab, act};

/// Sets the price a variant's reduction is announced against, or takes it
/// back when the field comes back empty.
#[route(POST)]
pub async fn compare_at(cx: &Cx, Form(form): Form<CompareAtForm>) -> Result<SeeOther> {
    let variant_id = variant_id(cx).to_owned();
    act(
        cx,
        Tab::Variants,
        "Reduction saved.",
        async |catalog, actor, id| {
            let amount = form.amount.trim();
            let amount = (!amount.is_empty())
                .then(|| parse_amount(amount))
                .transpose()?;
            catalog
                .set_compare_at(actor, id, Some(&variant_id), amount)
                .await
        },
    )
    .await
}
