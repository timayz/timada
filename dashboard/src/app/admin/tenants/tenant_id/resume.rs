use topcoat::{
    Result,
    context::Cx,
    router::{
        error::{SeeOther, see_other},
        path_param, route,
    },
};

use super::{TenantId, admin_tenant_url};
use crate::app::admin::require_admin;
use crate::app::tenants::{error_code, tenancy};

/// Puts a suspended tenant back online; its database opens on the next request.
#[route(POST)]
pub async fn resume(cx: &Cx) -> Result<SeeOther> {
    require_admin(cx).await?;
    let id = path_param::<TenantId>(cx);
    match tenancy(cx).resume_tenant(id).await {
        Ok(()) => Ok(see_other(admin_tenant_url(
            cx,
            id,
            None,
            Some("Tenant resumed."),
        ))),
        Err(err) if err.is_user_facing() => Ok(see_other(admin_tenant_url(
            cx,
            id,
            Some(error_code(&err)),
            None,
        ))),
        Err(err) => Err(err.into()),
    }
}
