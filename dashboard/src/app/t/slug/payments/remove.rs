use timada_tenancy::PERMISSION_PAYMENTS_MANAGE;
use topcoat::{
    Result,
    context::Cx,
    router::{
        error::{SeeOther, see_other},
        route,
    },
};

use super::payments_url;
use crate::app::t::slug::require_tenant;
use crate::app::tenants::{error_code, tenancy};

/// Stops taking payments: storefronts are refused from the next request to
/// pay. Orders already paid stay as they are.
#[route(POST)]
pub async fn remove(cx: &Cx) -> Result<SeeOther> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_PAYMENTS_MANAGE)
        .await?;
    let slug = access.tenant.slug.as_str();
    match tenancy(cx)
        .remove_payments(&access.user_id, &access.tenant.id)
        .await
    {
        Ok(()) => Ok(see_other(payments_url(
            cx,
            slug,
            None,
            Some("Payments stopped."),
        ))),
        Err(err) if err.is_user_facing() => Ok(see_other(payments_url(
            cx,
            slug,
            Some(error_code(&err)),
            None,
        ))),
        Err(err) => Err(err.into()),
    }
}
