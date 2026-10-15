use topcoat::{
    Result,
    context::Cx,
    router::{content::Form, error::SeeOther, route},
};

use super::{TaxCategoryForm, act};

/// Sets what kind of goods the product is, for VAT.
#[route(POST)]
pub async fn tax_category(cx: &Cx, Form(form): Form<TaxCategoryForm>) -> Result<SeeOther> {
    act(cx, "Kind of goods set.", async |catalog, actor, id| {
        catalog
            .set_product_tax_category(actor, id, Some(&form.category))
            .await
    })
    .await
}
