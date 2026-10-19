use serde::Deserialize;
use timada_tenancy::PERMISSION_ALIEXPRESS_MANAGE;
use topcoat::{
    Result,
    context::Cx,
    router::{
        content::Form,
        error::{SeeOther, see_other},
        route,
    },
};

use super::aliexpress_url;
use crate::app::t::slug::require_tenant;
use crate::app::tenants::{error_code, tenancy};

#[derive(Deserialize)]
pub struct ForwardingForm {
    auto_forward: Option<String>,
    try_to_pay: Option<String>,
}

/// What happens to the shop's paid orders with AliExpress items: placed
/// there as they come or by hand, paid by AliExpress or by the merchant.
#[route(POST)]
pub async fn forwarding(cx: &Cx, Form(form): Form<ForwardingForm>) -> Result<SeeOther> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_ALIEXPRESS_MANAGE)
        .await?;
    let slug = access.tenant.slug.as_str();
    match tenancy(cx)
        .set_aliexpress_forwarding(
            &access.user_id,
            &access.tenant.id,
            form.auto_forward.is_some(),
            form.try_to_pay.is_some(),
        )
        .await
    {
        Ok(()) => Ok(see_other(aliexpress_url(
            cx,
            slug,
            None,
            Some("Orders settings saved."),
        ))),
        Err(err) if err.is_user_facing() => Ok(see_other(aliexpress_url(
            cx,
            slug,
            Some(error_code(&err)),
            None,
        ))),
        Err(err) => Err(err.into()),
    }
}
