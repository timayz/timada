use timada_catalog::parse_amount;
use topcoat::{
    Result,
    context::Cx,
    router::{content::Form, error::SeeOther, route},
};

use super::{CompareAtForm, Tab, act};

/// Sets the price the product's reduction is announced against, or takes
/// it back when the field comes back empty.
#[route(POST)]
pub async fn compare_at(cx: &Cx, Form(form): Form<CompareAtForm>) -> Result<SeeOther> {
    act(
        cx,
        Tab::Overview,
        "Reduction saved.",
        async |catalog, actor, id| {
            let amount = form.amount.trim();
            let amount = (!amount.is_empty())
                .then(|| parse_amount(amount))
                .transpose()?;
            catalog.set_compare_at(actor, id, None, amount).await
        },
    )
    .await
}
