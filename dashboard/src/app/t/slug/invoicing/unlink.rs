use timada_tenancy::PERMISSION_TENANT_MANAGE;
use topcoat::{
    Result,
    context::Cx,
    router::{
        error::{SeeOther, see_other},
        route,
    },
};

use super::invoicing_url;
use crate::app::t::slug::require_tenant;
use crate::app::tenants::{error_code, tenancy};

/// Stops the tenant selling as its company: shoppers cannot pay until it
/// is linked again.
#[route(POST)]
pub async fn unlink(cx: &Cx) -> Result<SeeOther> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_TENANT_MANAGE)
        .await?;
    let slug = access.tenant.slug.as_str();
    match tenancy(cx)
        .unlink_tenant(&access.user_id, &access.tenant.id)
        .await
    {
        Ok(()) => Ok(see_other(invoicing_url(cx, slug, None, Some("Unlinked.")))),
        Err(err) if err.is_user_facing() => Ok(see_other(invoicing_url(
            cx,
            slug,
            Some(error_code(&err)),
            None,
        ))),
        Err(err) => Err(err.into()),
    }
}
