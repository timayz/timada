use serde::Deserialize;
use timada_catalog::{CatalogError, ShopCosts, parse_amount};
use timada_tenancy::PERMISSION_PRODUCTS_MANAGE;
use topcoat::{
    Result,
    context::Cx,
    router::{
        content::Form,
        error::{SeeOther, see_other},
        route,
    },
};

use super::{costs_url, parse_percent};
use crate::app::t::slug::products::{catalog, error_code};
use crate::app::t::slug::require_tenant;

/// The shop's sale costs as the merchant fills them in; a field left
/// empty is nothing.
#[derive(Deserialize)]
pub struct CostsForm {
    currency: String,
    #[serde(default)]
    shipping: String,
    #[serde(default)]
    import: String,
    #[serde(default)]
    fixed: String,
    #[serde(default)]
    fee_percent: String,
    #[serde(default)]
    fee_fixed: String,
}

impl CostsForm {
    fn costs(self) -> std::result::Result<ShopCosts, CatalogError> {
        let amount = |text: &str| match text.trim() {
            "" => Ok(0),
            text => parse_amount(text),
        };
        let payment_fee_bps = match self.fee_percent.trim() {
            "" => 0,
            text => parse_percent(text).ok_or(CatalogError::InvalidFee)?,
        };
        Ok(ShopCosts {
            shipping_cents: amount(&self.shipping)?,
            import_cents: amount(&self.import)?,
            fixed_cents: amount(&self.fixed)?,
            payment_fee_bps,
            payment_fee_cents: amount(&self.fee_fixed)?,
            currency: self.currency,
        })
    }
}

/// Says what a sale costs the shop beyond the goods.
#[route(POST)]
pub async fn set(cx: &Cx, Form(form): Form<CostsForm>) -> Result<SeeOther> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_PRODUCTS_MANAGE)
        .await?;
    let slug = access.tenant.slug.as_str();
    let set = match form.costs() {
        Ok(costs) => {
            catalog(&access)?
                .set_sale_costs(&access.user_id, costs)
                .await
        }
        Err(err) => Err(err),
    };
    match set {
        Ok(()) => Ok(see_other(costs_url(cx, slug, None, Some("Costs saved.")))),
        Err(err) if err.is_user_facing() => {
            Ok(see_other(costs_url(cx, slug, Some(error_code(&err)), None)))
        }
        Err(err) => Err(topcoat::Error::from_anyhow(err.into())),
    }
}
