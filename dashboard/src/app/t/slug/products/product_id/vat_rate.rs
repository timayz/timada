use timada_catalog::CatalogError;
use topcoat::{
    Result,
    context::Cx,
    router::{content::Form, error::SeeOther, route},
};

use super::{VatRateForm, act};

/// Sets the VAT rate; blank falls back to the merchant's default.
#[route(POST)]
pub async fn vat_rate(cx: &Cx, Form(form): Form<VatRateForm>) -> Result<SeeOther> {
    act(cx, "VAT rate set.", async |catalog, actor, id| {
        let rate = match form.rate.trim() {
            "" => None,
            rate => Some(rate.parse().map_err(|_| CatalogError::InvalidVatRate)?),
        };
        catalog.set_product_vat_rate(actor, id, rate).await
    })
    .await
}
