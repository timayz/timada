use timada_tenancy::PERMISSION_ALIEXPRESS_MANAGE;
use topcoat::{
    Result,
    context::Cx,
    router::{content::Form, error::SeeOther, path_param, route},
};

use super::{LinkId, back_to};
use crate::app::t::slug::aliexpress::PricingForm;
use crate::app::t::slug::require_tenant;

#[derive(serde::Deserialize)]
pub struct ProductPricingForm {
    #[serde(default)]
    back: String,
    #[serde(flatten)]
    pricing: PricingForm,
}

/// Gives the product its own price rule (blank: the shop's). It applies at
/// the next read.
#[route(POST)]
pub async fn pricing(cx: &Cx, Form(form): Form<ProductPricingForm>) -> Result<SeeOther> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_ALIEXPRESS_MANAGE)
        .await?;
    let link_id = path_param::<LinkId>(cx).to_owned();
    let rule = match form.pricing.rule() {
        Ok(rule) => rule,
        Err(code) => return Ok(back_to(cx, &access, &form.back, Some(code), None)),
    };
    let links = timada_aliexpress::Command(access.executor()?.clone());
    match links.set_pricing(&access.user_id, &link_id, rule).await {
        Ok(()) => Ok(back_to(
            cx,
            &access,
            &form.back,
            None,
            Some("Price rule saved: it applies at the next read."),
        )),
        Err(err) => match err.code() {
            Some(code) => Ok(back_to(cx, &access, &form.back, Some(code), None)),
            None => Err(topcoat::Error::from_anyhow(err.into())),
        },
    }
}
