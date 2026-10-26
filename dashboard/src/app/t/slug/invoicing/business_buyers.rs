use timada_tenancy::PERMISSION_TENANT_MANAGE;
use topcoat::{
    Result,
    context::Cx,
    router::{
        content::Form,
        error::{SeeOther, see_other},
        route,
    },
};

use super::invoicing_url;
use crate::app::t::slug::require_tenant;

#[derive(serde::Deserialize)]
pub struct BusinessBuyersForm {
    /// `on` to sell to businesses, anything else to stop.
    accept: String,
}

/// Opens or closes the shop to business buyers: a shopper who gives an EU
/// VAT number, invoiced without VAT under reverse charge.
#[route(POST)]
pub async fn business_buyers(cx: &Cx, Form(form): Form<BusinessBuyersForm>) -> Result<SeeOther> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_TENANT_MANAGE)
        .await?;
    let accept = form.accept == "on";
    timada_orders::Command::new(access.executor()?.clone(), access.tenant_id())
        .set_business_buyers(&access.user_id, accept)
        .await
        .map_err(|err| topcoat::Error::from_anyhow(err.into()))?;
    let message = if accept {
        "Business buyers accepted."
    } else {
        "Business buyers no longer accepted."
    };
    Ok(see_other(invoicing_url(
        cx,
        &access.tenant.slug,
        None,
        Some(message),
    )))
}
