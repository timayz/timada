use timada_tenancy::PERMISSION_SHIPPING_MANAGE;
use topcoat::{
    Result,
    context::Cx,
    router::{
        content::Form,
        error::{SeeOther, see_other},
        route,
    },
};

use super::shipping_url;
use crate::app::t::slug::products::product_id::DeliveryDaysForm;
use crate::app::t::slug::products::{catalog, error_code};
use crate::app::t::slug::require_tenant;

/// Sets the delivery time of the products that give none.
#[route(POST)]
pub async fn delivery_days(cx: &Cx, Form(form): Form<DeliveryDaysForm>) -> Result<SeeOther> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_SHIPPING_MANAGE)
        .await?;
    let slug = access.tenant.slug.as_str();
    let set = match timada_catalog::DeliveryDays::parse(&form.min, &form.max) {
        Ok(days) => {
            catalog(&access)?
                .set_default_delivery_days(&access.user_id, days)
                .await
        }
        Err(err) => Err(err),
    };
    match set {
        Ok(()) => Ok(see_other(shipping_url(
            cx,
            slug,
            None,
            Some("Default delivery time set."),
        ))),
        Err(err) if err.is_user_facing() => Ok(see_other(shipping_url(
            cx,
            slug,
            Some(error_code(&err)),
            None,
        ))),
        Err(err) => Err(topcoat::Error::from_anyhow(err.into())),
    }
}
