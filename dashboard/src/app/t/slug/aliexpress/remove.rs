use timada_tenancy::PERMISSION_ALIEXPRESS_MANAGE;
use topcoat::{
    Result,
    context::Cx,
    router::{
        error::{SeeOther, see_other},
        route,
    },
};

use super::aliexpress_url;
use crate::app::t::slug::require_tenant;
use crate::app::tenants::{error_code, tenancy};

/// Forgets the shop's app and account: nothing is read from AliExpress any
/// more. The imported products stay as they are.
#[route(POST)]
pub async fn remove(cx: &Cx) -> Result<SeeOther> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_ALIEXPRESS_MANAGE)
        .await?;
    let slug = access.tenant.slug.as_str();
    match tenancy(cx)
        .remove_aliexpress(&access.user_id, &access.tenant.id)
        .await
    {
        Ok(()) => Ok(see_other(aliexpress_url(
            cx,
            slug,
            None,
            Some("AliExpress disconnected."),
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
